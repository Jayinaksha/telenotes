mod config;
mod db;
mod handlers;
mod models;
mod notion;
mod storage;

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use deadpool_postgres::Pool;
use teloxide::prelude::*;
use teloxide::types::{BotCommand, Update};

use config::Config;
use notion::NotionClient;
use storage::{GitHubStore, R2Store, Vault};

/// Telegram echoes this header back on every webhook delivery. Checking it is
/// what stops anyone who guesses the URL from injecting fake updates.
const SECRET_HEADER: &str = "x-telegram-bot-api-secret-token";

#[derive(Clone)]
pub struct AppState {
    pub bot: Bot,
    pub config: Arc<Config>,
    pub db: Pool,
    pub vault: Vault,
    pub notion: Option<NotionClient>,
    pub http: reqwest::Client,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,telenotes_bot=debug".into()),
        )
        .init();

    let config = Config::from_env()?;

    if config.allowed_chat_ids.is_empty() {
        tracing::warn!(
            "ALLOWED_CHAT_IDS is unset — anyone who finds this bot can store files in \
             your vault. Message the bot and it will tell you your chat id."
        );
    }
    if config.r2.is_none() {
        tracing::info!("R2 not configured; files will be stored in the GitHub vault");
    }

    let bot = Bot::new(&config.bot_token);
    let http = reqwest::Client::builder()
        .user_agent("telenotes-bot")
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let db_pool = db::create_pool(&config.database_url)?;
    // Fail fast on a bad connection string rather than at the first message.
    db::health_check(&db_pool).await?;
    tracing::info!("database reachable");

    let github = GitHubStore::new(
        http.clone(),
        config.github.token.clone(),
        config.github.owner.clone(),
        config.github.repo.clone(),
        config.github.branch.clone(),
    );
    let r2 = config
        .r2
        .as_ref()
        .map(|c| {
            R2Store::new(
                http.clone(),
                &c.account_id,
                &c.access_key_id,
                &c.secret_access_key,
                &c.bucket,
            )
        })
        .transpose()?;
    let vault = Vault::new(github, r2);

    let notion = config
        .notion
        .as_ref()
        .map(|c| NotionClient::new(http.clone(), c.token.clone(), c.database_id.clone()));
    if notion.is_some() {
        tracing::info!("Notion mirroring enabled");
    }

    let port = config.port;
    let webhook_url = format!("{}/webhook", config.webhook_url.trim_end_matches('/'));

    // Registering the secret with `set_webhook` is what makes the header
    // check above meaningful; the secret never appears in the URL, so it
    // can't leak through logs or referrers.
    bot.set_webhook(webhook_url.parse()?)
        .secret_token(config.webhook_secret.clone())
        .drop_pending_updates(true)
        .await?;
    tracing::info!("webhook registered at {webhook_url}");

    if let Err(e) = bot.set_my_commands(bot_commands()).await {
        tracing::warn!("could not register command menu: {e:?}");
    }

    let state = AppState {
        bot,
        config: Arc::new(config),
        db: db_pool,
        vault,
        notion,
        http,
    };

    let app = Router::new()
        .route("/webhook", post(webhook_handler))
        .route("/health", get(health_handler))
        // Reached by plain browser navigation from the dashboard, so no CORS
        // preflight is involved.
        .route("/d/{token}", get(download_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("listening on 0.0.0.0:{port}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn bot_commands() -> Vec<BotCommand> {
    vec![
        BotCommand::new("notes", "List your notes"),
        BotCommand::new("files", "List your stored files"),
        BotCommand::new("search", "Full-text search everything"),
        BotCommand::new("tags", "List tags in use"),
        BotCommand::new("get", "Resend a note or file by id"),
        BotCommand::new("delete", "Delete an item and its file"),
        BotCommand::new("stats", "What you've captured so far"),
        BotCommand::new("link", "Get a code for the web dashboard"),
        BotCommand::new("help", "Show help"),
    ]
}

async fn webhook_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(update): Json<Update>,
) -> StatusCode {
    let presented = headers.get(SECRET_HEADER).and_then(|v| v.to_str().ok());
    if presented != Some(state.config.webhook_secret.as_str()) {
        tracing::warn!("rejected webhook delivery with bad or missing secret token");
        return StatusCode::FORBIDDEN;
    }

    // Answer immediately and process out of band: Telegram retries anything
    // it considers slow, which would duplicate saves.
    tokio::spawn(handlers::handle_update(state, update));
    StatusCode::OK
}

/// Pinged by the keep-alive cron. Touching Postgres is the point — it resets
/// Supabase's 7-day inactivity timer as well as waking the web service.
async fn health_handler(State(state): State<AppState>) -> StatusCode {
    match db::health_check(&state.db).await {
        Ok(()) => {
            if let Err(e) = db::purge_expired(&state.db).await {
                tracing::warn!("expiry cleanup failed: {e:?}");
            }
            StatusCode::OK
        }
        Err(e) => {
            tracing::error!("health check failed: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// Trades a token minted by Postgres' `issue_download_token` for the file.
///
/// The token *is* the authorization: it only exists because an authenticated
/// user passed the RLS-backed ownership check in the database. For R2 we hand
/// back a redirect to a presigned URL so the bytes never transit this service;
/// GitHub-backed items are streamed through.
async fn download_handler(State(state): State<AppState>, Path(token): Path<String>) -> Response {
    let item = match db::item_for_download_token(&state.db, &token).await {
        Ok(Some(item)) => item,
        Ok(None) => return (StatusCode::NOT_FOUND, "unknown or expired link").into_response(),
        Err(e) => {
            tracing::error!("download token lookup failed: {e:?}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "lookup failed").into_response();
        }
    };

    let Some(path) = item.storage_path.clone() else {
        return (StatusCode::NOT_FOUND, "item has no stored file").into_response();
    };

    if let Some(url) =
        state
            .vault
            .presigned_get(item.backend(), &path, std::time::Duration::from_secs(300))
    {
        return Redirect::temporary(&url).into_response();
    }

    match state.vault.get(item.backend(), &path).await {
        Ok(bytes) => {
            let name = item.file_name.unwrap_or_else(|| "file".to_string());
            let mime = item
                .mime_type
                .unwrap_or_else(|| "application/octet-stream".to_string());
            let mut headers = HeaderMap::new();
            if let Ok(v) = mime.parse() {
                headers.insert(axum::http::header::CONTENT_TYPE, v);
            }
            // The name is already sanitised to an ASCII-safe set on the way in.
            if let Ok(v) = format!("attachment; filename=\"{name}\"").parse() {
                headers.insert(axum::http::header::CONTENT_DISPOSITION, v);
            }
            (headers, bytes).into_response()
        }
        Err(e) => {
            tracing::error!("vault fetch failed for {}: {e:?}", item.short_id());
            (StatusCode::BAD_GATEWAY, "could not fetch file").into_response()
        }
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
