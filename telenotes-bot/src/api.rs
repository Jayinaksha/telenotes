//! HTTP write API for the dashboard and TUI.
//!
//! Every route trades a Postgres-minted, single-use `action_tokens` row for
//! one specific write, mirroring `download_handler`'s token-for-bytes trade
//! (see `main.rs`) exactly. The token is the only thing authenticating these
//! routes -- the bot never verifies a Supabase JWT, because Postgres already
//! proved ownership (via RLS + `auth.uid()`) before minting the token in
//! `issue_write_token`.

use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use telenotes_core::text::sanitize;
use uuid::Uuid;

use crate::db;
use crate::handlers;
use crate::AppState;

pub(crate) enum ApiError {
    BadRequest(String),
    Unauthorized,
    Forbidden,
    NotFound,
    Internal(anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg).into_response(),
            ApiError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "invalid, expired, or already-used token",
            )
                .into_response(),
            ApiError::Forbidden => {
                (StatusCode::FORBIDDEN, "token does not match this item").into_response()
            }
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found").into_response(),
            ApiError::Internal(e) => {
                tracing::error!("api request failed: {e:?}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
            }
        }
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError::Internal(e)
    }
}

type ApiResult<T> = Result<T, ApiError>;

const ALLOWED_UPLOAD_KINDS: &[&str] = &["document", "photo", "voice", "video", "audio"];

#[derive(Deserialize)]
pub struct CreateNoteBody {
    token: String,
    text: String,
}

pub async fn create_note_handler(
    State(state): State<AppState>,
    Json(body): Json<CreateNoteBody>,
) -> ApiResult<Response> {
    if body.text.trim().is_empty() {
        return Err(ApiError::BadRequest("text must not be empty".to_string()));
    }

    let Some(claimed) = db::claim_write_token(&state.db, &body.token, "create_note").await? else {
        return Err(ApiError::Unauthorized);
    };

    let item = handlers::create_note(&state, claimed.user_id, &body.text).await?;
    Ok((StatusCode::CREATED, Json(item)).into_response())
}

#[derive(Deserialize)]
pub struct EditItemBody {
    token: String,
    content_text: Option<String>,
    tags: Option<Vec<String>>,
    file_name: Option<String>,
    mime_type: Option<String>,
}

pub async fn edit_item_handler(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
    Json(body): Json<EditItemBody>,
) -> ApiResult<Response> {
    if body.content_text.is_none()
        && body.tags.is_none()
        && body.file_name.is_none()
        && body.mime_type.is_none()
    {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }

    let Some(claimed) = db::claim_write_token(&state.db, &body.token, "edit_item").await? else {
        return Err(ApiError::Unauthorized);
    };
    if claimed.item_id != Some(item_id) {
        return Err(ApiError::Forbidden);
    }

    let updated = handlers::edit_item(
        &state,
        claimed.user_id,
        item_id,
        body.content_text.as_deref(),
        body.tags.as_deref(),
        body.file_name.as_deref(),
        body.mime_type.as_deref(),
    )
    .await?;

    match updated {
        Some(item) => Ok(Json(item).into_response()),
        // Only reachable if the item was deleted between token mint and
        // claim -- the token itself already proved ownership.
        None => Err(ApiError::NotFound),
    }
}

#[derive(Deserialize)]
pub struct DeleteItemQuery {
    token: String,
}

pub async fn delete_item_handler(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
    Query(query): Query<DeleteItemQuery>,
) -> ApiResult<Response> {
    let Some(claimed) = db::claim_write_token(&state.db, &query.token, "delete_item").await? else {
        return Err(ApiError::Unauthorized);
    };
    if claimed.item_id != Some(item_id) {
        return Err(ApiError::Forbidden);
    }

    let deleted = handlers::delete_item(&state, claimed.user_id, item_id).await?;
    match deleted {
        Some(item) => Ok(Json(item).into_response()),
        None => Err(ApiError::NotFound),
    }
}

/// Multipart fields: `token`, `kind` (one of `ALLOWED_UPLOAD_KINDS`), `file`
/// (binary, with a filename), `caption` (optional).
pub async fn upload_file_handler(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> ApiResult<Response> {
    let mut token: Option<String> = None;
    let mut kind: Option<String> = None;
    let mut caption: Option<String> = None;
    let mut file_name: Option<String> = None;
    let mut mime_type: Option<String> = None;
    let mut file_bytes: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("malformed multipart body: {e}")))?
    {
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "token" => {
                token = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::BadRequest(e.to_string()))?,
                );
            }
            "kind" => {
                kind = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::BadRequest(e.to_string()))?,
                );
            }
            "caption" => {
                caption = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::BadRequest(e.to_string()))?,
                );
            }
            "file" => {
                file_name = field.file_name().map(str::to_string);
                mime_type = field.content_type().map(str::to_string);
                file_bytes = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| ApiError::BadRequest(e.to_string()))?
                        .to_vec(),
                );
            }
            _ => {}
        }
    }

    let token = token.ok_or_else(|| ApiError::BadRequest("missing token field".to_string()))?;
    let kind = kind.ok_or_else(|| ApiError::BadRequest("missing kind field".to_string()))?;
    if !ALLOWED_UPLOAD_KINDS.contains(&kind.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "kind must be one of {}",
            ALLOWED_UPLOAD_KINDS.join(", ")
        )));
    }
    let bytes = file_bytes.ok_or_else(|| ApiError::BadRequest("missing file field".to_string()))?;
    let sanitized_name = sanitize(&file_name.unwrap_or_else(|| "file".to_string()));

    let Some(claimed) = db::claim_write_token(&state.db, &token, "upload_file").await? else {
        return Err(ApiError::Unauthorized);
    };

    // Keyed by a fresh id rather than a Telegram unique_id -- these uploads
    // have no Telegram provenance.
    let storage_path = format!(
        "files/{}/{}_{sanitized_name}",
        claimed.user_id,
        Uuid::new_v4()
    );

    let item = handlers::create_file(
        &state,
        claimed.user_id,
        &kind,
        &storage_path,
        bytes,
        &sanitized_name,
        mime_type.as_deref(),
        caption.as_deref(),
        None,
        None,
    )
    .await?;

    Ok((StatusCode::CREATED, Json(item)).into_response())
}
