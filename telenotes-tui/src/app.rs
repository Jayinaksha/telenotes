use std::time::{Duration, Instant};

use crossterm::event::KeyCode;
use ratatui::text::Line;
use telenotes_core::Item;
use uuid::Uuid;

use crate::botclient::BotClient;
use crate::html;
use crate::markdown;
use crate::session::Session;
use crate::supabase::{DayCount, StorageRow, SupabaseClient, TagCount};

pub const UPLOAD_KINDS: [&str; 5] = ["document", "photo", "voice", "video", "audio"];

/// How long a status message (e.g. "Note saved.") stays before the key-hint
/// bar takes over the status line again. Without this it stuck around
/// forever -- the first action of the session would permanently bury the
/// hints, since almost every action sets a status and next to nothing ever
/// cleared it.
const STATUS_TTL: Duration = Duration::from_secs(4);

pub enum Screen {
    Login,
    List,
    Detail,
    Search,
    AddNote,
    UploadFile,
    EditItem,
    Stats,
}

pub enum LoginStage {
    Email,
    Code,
}

#[derive(PartialEq, Eq)]
pub enum UploadFocus {
    Path,
    Kind,
    Caption,
}

#[derive(PartialEq, Eq)]
pub enum EditFocus {
    Text,
    Tags,
    FileName,
    MimeType,
}

pub struct App {
    supabase: SupabaseClient,
    bot: BotClient,
    session: Option<Session>,
    pub screen: Screen,
    pub should_quit: bool,
    pub status: Option<String>,
    status_set_at: Option<Instant>,

    pub login_stage: LoginStage,
    pub login_email: String,
    pub login_code: String,

    pub items: Vec<Item>,
    pub selected: usize,
    pending_delete: Option<Uuid>,
    /// Rendered markdown for the item currently open in Detail, when it's a
    /// document that looks like one -- `None` otherwise, including while a
    /// fetch is still in flight (the key handler blocks on it, same as
    /// every other network call in this app; see `run()` in main.rs).
    pub detail_body: Option<Vec<Line<'static>>>,

    pub search_query: String,

    pub tags: Vec<TagCount>,
    pub days: Vec<DayCount>,
    pub storage: Vec<StorageRow>,

    pub note_text: String,

    pub upload_path: String,
    pub upload_kind_idx: usize,
    pub upload_caption: String,
    pub upload_focus: UploadFocus,

    editing_item_id: Option<Uuid>,
    pub edit_text: String,
    pub edit_tags: String,
    pub edit_file_name: String,
    pub edit_mime_type: String,
    pub edit_focus: EditFocus,
}

impl App {
    pub fn new(supabase: SupabaseClient, bot: BotClient) -> Self {
        Self {
            supabase,
            bot,
            session: None,
            screen: Screen::Login,
            should_quit: false,
            status: None,
            status_set_at: None,
            login_stage: LoginStage::Email,
            login_email: String::new(),
            login_code: String::new(),
            items: Vec::new(),
            selected: 0,
            pending_delete: None,
            detail_body: None,
            search_query: String::new(),
            tags: Vec::new(),
            days: Vec::new(),
            storage: Vec::new(),
            note_text: String::new(),
            upload_path: String::new(),
            upload_kind_idx: 0,
            upload_caption: String::new(),
            upload_focus: UploadFocus::Path,
            editing_item_id: None,
            edit_text: String::new(),
            edit_tags: String::new(),
            edit_file_name: String::new(),
            edit_mime_type: String::new(),
            edit_focus: EditFocus::Text,
        }
    }

    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some(message.into());
        self.status_set_at = Some(Instant::now());
    }

    pub fn clear_status(&mut self) {
        self.status = None;
        self.status_set_at = None;
    }

    /// Called once per event-loop tick regardless of input, so a status
    /// message fades back to the key-hint bar on its own after `STATUS_TTL`
    /// even if the user never presses another key.
    pub fn expire_status(&mut self) {
        if self
            .status_set_at
            .is_some_and(|t| t.elapsed() >= STATUS_TTL)
        {
            self.clear_status();
        }
    }

    /// Tries a saved session on startup; falls back to the login screen
    /// (untouched) on any failure.
    pub async fn bootstrap(&mut self) {
        let Some(session) = Session::load().await else {
            return;
        };
        let session = if session.is_near_expiry() {
            match self.supabase.refresh(&session).await {
                Ok(refreshed) => {
                    let _ = refreshed.save().await;
                    refreshed
                }
                Err(_) => return,
            }
        } else {
            session
        };
        self.session = Some(session);
        self.screen = Screen::List;
        self.refresh_items().await;
    }

    pub async fn handle_key(&mut self, code: KeyCode) {
        match self.screen {
            Screen::Login => self.handle_login_key(code).await,
            Screen::List => self.handle_list_key(code).await,
            Screen::Detail => handle_esc_back(code, &mut self.screen, Screen::List),
            Screen::Search => self.handle_search_key(code).await,
            Screen::AddNote => self.handle_add_note_key(code).await,
            Screen::UploadFile => self.handle_upload_key(code).await,
            Screen::EditItem => self.handle_edit_key(code).await,
            Screen::Stats => handle_esc_back(code, &mut self.screen, Screen::List),
        }
    }

    /// Refreshes the session if it's about to expire. Returns `false` (and
    /// bounces to the login screen) if that fails -- callers should bail out
    /// of whatever they were doing.
    async fn ensure_fresh_session(&mut self) -> bool {
        let Some(session) = self.session.clone() else {
            self.screen = Screen::Login;
            return false;
        };
        if !session.is_near_expiry() {
            return true;
        }
        match self.supabase.refresh(&session).await {
            Ok(refreshed) => {
                let _ = refreshed.save().await;
                self.session = Some(refreshed);
                true
            }
            Err(e) => {
                self.set_status(format!("session expired, please log in again: {e}"));
                self.session = None;
                self.screen = Screen::Login;
                false
            }
        }
    }

    async fn refresh_items(&mut self) {
        if !self.ensure_fresh_session().await {
            return;
        }
        let session = self.session.clone().unwrap();
        match self.supabase.list_items(&session).await {
            Ok(items) => {
                self.items = items;
                self.selected = self.selected.min(self.items.len().saturating_sub(1));
            }
            Err(e) => self.set_status(format!("could not load items: {e}")),
        }
    }

    async fn load_stats(&mut self) {
        if !self.ensure_fresh_session().await {
            return;
        }
        let session = self.session.clone().unwrap();
        match self.supabase.tag_counts(&session).await {
            Ok(tags) => self.tags = tags,
            Err(e) => {
                self.set_status(format!("could not load tags: {e}"));
                return;
            }
        }
        match self.supabase.items_per_day(&session, 30).await {
            Ok(days) => self.days = days,
            Err(e) => {
                self.set_status(format!("could not load activity: {e}"));
                return;
            }
        }
        match self.supabase.storage_breakdown(&session).await {
            Ok(rows) => self.storage = rows,
            Err(e) => {
                self.set_status(format!("could not load storage breakdown: {e}"));
                return;
            }
        }
        self.screen = Screen::Stats;
    }

    async fn handle_login_key(&mut self, code: KeyCode) {
        match self.login_stage {
            LoginStage::Email => match code {
                KeyCode::Enter => {
                    if self.login_email.trim().is_empty() {
                        return;
                    }
                    match self.supabase.request_otp(self.login_email.trim()).await {
                        Ok(()) => {
                            self.set_status("Code sent -- check your email.".to_string());
                            self.login_stage = LoginStage::Code;
                        }
                        Err(e) => self.set_status(format!("could not send code: {e}")),
                    }
                }
                KeyCode::Char(c) => self.login_email.push(c),
                KeyCode::Backspace => {
                    self.login_email.pop();
                }
                KeyCode::Esc => self.should_quit = true,
                _ => {}
            },
            LoginStage::Code => match code {
                KeyCode::Enter => {
                    let email = self.login_email.trim().to_string();
                    let otp_code = self.login_code.trim().to_string();
                    match self.supabase.verify_otp(&email, &otp_code).await {
                        Ok(session) => {
                            let _ = session.save().await;
                            self.session = Some(session);
                            self.login_code.clear();
                            self.clear_status();
                            self.screen = Screen::List;
                            self.refresh_items().await;
                        }
                        Err(e) => self.set_status(format!("login failed: {e}")),
                    }
                }
                KeyCode::Char(c) => self.login_code.push(c),
                KeyCode::Backspace => {
                    self.login_code.pop();
                }
                KeyCode::Esc => {
                    self.login_stage = LoginStage::Email;
                    self.login_code.clear();
                }
                _ => {}
            },
        }
    }

    async fn handle_list_key(&mut self, code: KeyCode) {
        if !matches!(code, KeyCode::Char('x')) {
            self.pending_delete = None;
        }
        match code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.items.is_empty() {
                    self.selected = (self.selected + 1).min(self.items.len() - 1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Enter => {
                if !self.items.is_empty() {
                    self.screen = Screen::Detail;
                    self.load_detail_body().await;
                }
            }
            KeyCode::Char('/') => {
                self.search_query.clear();
                self.screen = Screen::Search;
            }
            KeyCode::Char('r') => self.refresh_items().await,
            KeyCode::Char('n') => {
                self.note_text.clear();
                self.screen = Screen::AddNote;
            }
            KeyCode::Char('u') => {
                self.upload_path.clear();
                self.upload_caption.clear();
                self.upload_kind_idx = 0;
                self.upload_focus = UploadFocus::Path;
                self.screen = Screen::UploadFile;
            }
            KeyCode::Char('e') => {
                if let Some(item) = self.items.get(self.selected) {
                    self.edit_text = item.content_text.clone().unwrap_or_default();
                    self.edit_tags = item.tags.join(",");
                    self.edit_file_name = item.file_name.clone().unwrap_or_default();
                    self.edit_mime_type = item.mime_type.clone().unwrap_or_default();
                    self.editing_item_id = Some(item.id);
                    self.edit_focus = EditFocus::Text;
                    self.screen = Screen::EditItem;
                }
            }
            KeyCode::Char('x') => {
                if let Some(item) = self.items.get(self.selected) {
                    if self.pending_delete == Some(item.id) {
                        self.pending_delete = None;
                        self.delete_selected().await;
                    } else {
                        self.pending_delete = Some(item.id);
                        self.set_status(
                            "Press x again to confirm delete, any other key cancels.".to_string(),
                        );
                    }
                }
            }
            KeyCode::Char('d') => self.download_selected().await,
            KeyCode::Char('t') => self.load_stats().await,
            _ => {}
        }
    }

    async fn handle_search_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Enter => {
                if !self.ensure_fresh_session().await {
                    return;
                }
                let session = self.session.clone().unwrap();
                match self
                    .supabase
                    .search_items(&session, self.search_query.trim())
                    .await
                {
                    Ok(items) => {
                        self.items = items;
                        self.selected = 0;
                        self.screen = Screen::List;
                    }
                    Err(e) => self.set_status(format!("search failed: {e}")),
                }
            }
            KeyCode::Char(c) => self.search_query.push(c),
            KeyCode::Backspace => {
                self.search_query.pop();
            }
            KeyCode::Esc => self.screen = Screen::List,
            _ => {}
        }
    }

    async fn handle_add_note_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Enter => {
                if self.note_text.trim().is_empty() {
                    return;
                }
                if !self.ensure_fresh_session().await {
                    return;
                }
                let session = self.session.clone().unwrap();
                let token = match self
                    .supabase
                    .issue_write_token(&session, "create_note", None)
                    .await
                {
                    Ok(t) => t,
                    Err(e) => {
                        self.set_status(format!("could not save note: {e}"));
                        return;
                    }
                };
                match self.bot.create_note(&token, &self.note_text).await {
                    Ok(_) => {
                        self.set_status("Note saved.".to_string());
                        self.note_text.clear();
                        self.screen = Screen::List;
                        self.refresh_items().await;
                    }
                    Err(e) => self.set_status(format!("could not save note: {e}")),
                }
            }
            KeyCode::Char(c) => self.note_text.push(c),
            KeyCode::Backspace => {
                self.note_text.pop();
            }
            KeyCode::Esc => self.screen = Screen::List,
            _ => {}
        }
    }

    async fn handle_upload_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Tab => {
                self.upload_focus = match self.upload_focus {
                    UploadFocus::Path => UploadFocus::Kind,
                    UploadFocus::Kind => UploadFocus::Caption,
                    UploadFocus::Caption => UploadFocus::Path,
                };
            }
            KeyCode::Left if self.upload_focus == UploadFocus::Kind => {
                self.upload_kind_idx =
                    (self.upload_kind_idx + UPLOAD_KINDS.len() - 1) % UPLOAD_KINDS.len();
            }
            KeyCode::Right if self.upload_focus == UploadFocus::Kind => {
                self.upload_kind_idx = (self.upload_kind_idx + 1) % UPLOAD_KINDS.len();
            }
            KeyCode::Enter => self.submit_upload().await,
            KeyCode::Char(c) => match self.upload_focus {
                UploadFocus::Path => self.upload_path.push(c),
                UploadFocus::Caption => self.upload_caption.push(c),
                UploadFocus::Kind => {}
            },
            KeyCode::Backspace => match self.upload_focus {
                UploadFocus::Path => {
                    self.upload_path.pop();
                }
                UploadFocus::Caption => {
                    self.upload_caption.pop();
                }
                UploadFocus::Kind => {}
            },
            KeyCode::Esc => self.screen = Screen::List,
            _ => {}
        }
    }

    async fn submit_upload(&mut self) {
        let path = self.upload_path.trim().to_string();
        if path.is_empty() {
            self.set_status("enter a file path first".to_string());
            return;
        }
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(e) => {
                self.set_status(format!("could not read {path}: {e}"));
                return;
            }
        };
        let file_name = std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        let kind = UPLOAD_KINDS[self.upload_kind_idx];

        if !self.ensure_fresh_session().await {
            return;
        }
        let session = self.session.clone().unwrap();
        let token = match self
            .supabase
            .issue_write_token(&session, "upload_file", None)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                self.set_status(format!("could not upload: {e}"));
                return;
            }
        };
        let caption = (!self.upload_caption.trim().is_empty())
            .then(|| self.upload_caption.trim().to_string());
        match self
            .bot
            .upload_file(&token, kind, bytes, &file_name, caption.as_deref())
            .await
        {
            Ok(_) => {
                self.set_status("File uploaded.".to_string());
                self.upload_path.clear();
                self.upload_caption.clear();
                self.screen = Screen::List;
                self.refresh_items().await;
            }
            Err(e) => self.set_status(format!("could not upload: {e}")),
        }
    }

    async fn handle_edit_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Tab => {
                self.edit_focus = match self.edit_focus {
                    EditFocus::Text => EditFocus::Tags,
                    EditFocus::Tags => EditFocus::FileName,
                    EditFocus::FileName => EditFocus::MimeType,
                    EditFocus::MimeType => EditFocus::Text,
                };
            }
            KeyCode::Enter => self.submit_edit().await,
            KeyCode::Char(c) => match self.edit_focus {
                EditFocus::Text => self.edit_text.push(c),
                EditFocus::Tags => self.edit_tags.push(c),
                EditFocus::FileName => self.edit_file_name.push(c),
                EditFocus::MimeType => self.edit_mime_type.push(c),
            },
            KeyCode::Backspace => match self.edit_focus {
                EditFocus::Text => {
                    self.edit_text.pop();
                }
                EditFocus::Tags => {
                    self.edit_tags.pop();
                }
                EditFocus::FileName => {
                    self.edit_file_name.pop();
                }
                EditFocus::MimeType => {
                    self.edit_mime_type.pop();
                }
            },
            KeyCode::Esc => self.screen = Screen::List,
            _ => {}
        }
    }

    async fn submit_edit(&mut self) {
        let Some(item_id) = self.editing_item_id else {
            return;
        };
        if !self.ensure_fresh_session().await {
            return;
        }
        let session = self.session.clone().unwrap();
        let token = match self
            .supabase
            .issue_write_token(&session, "edit_item", Some(item_id))
            .await
        {
            Ok(t) => t,
            Err(e) => {
                self.set_status(format!("could not edit: {e}"));
                return;
            }
        };
        let tags: Vec<String> = self
            .edit_tags
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        let text = self.edit_text.clone();
        let file_name =
            (!self.edit_file_name.trim().is_empty()).then_some(self.edit_file_name.trim());
        let mime_type =
            (!self.edit_mime_type.trim().is_empty()).then_some(self.edit_mime_type.trim());
        match self
            .bot
            .edit_item(
                &token,
                item_id,
                Some(&text),
                Some(&tags),
                file_name,
                mime_type,
            )
            .await
        {
            Ok(_) => {
                self.set_status("Saved.".to_string());
                self.screen = Screen::List;
                self.refresh_items().await;
            }
            Err(e) => self.set_status(format!("could not edit: {e}")),
        }
    }

    async fn delete_selected(&mut self) {
        let Some(item) = self.items.get(self.selected).cloned() else {
            return;
        };
        if !self.ensure_fresh_session().await {
            return;
        }
        let session = self.session.clone().unwrap();
        let token = match self
            .supabase
            .issue_write_token(&session, "delete_item", Some(item.id))
            .await
        {
            Ok(t) => t,
            Err(e) => {
                self.set_status(format!("could not delete: {e}"));
                return;
            }
        };
        match self.bot.delete_item(&token, item.id).await {
            Ok(_) => {
                self.set_status(format!("Deleted {}.", item.short_id()));
                self.refresh_items().await;
            }
            Err(e) => self.set_status(format!("could not delete: {e}")),
        }
    }

    /// Fetches and renders a document's own text when it looks like
    /// markdown or HTML, so Detail can show more than just the file name.
    /// Notes already show their text via `content_text` (no fetch needed);
    /// any other document kind (PDF, image, ...) has no plain-text
    /// rendering story here, so `detail_body` just stays `None` for those.
    async fn load_detail_body(&mut self) {
        self.detail_body = None;
        let Some(item) = self.items.get(self.selected).cloned() else {
            return;
        };
        if item.kind == "note" {
            return;
        }
        let file_name = item.file_name.as_deref();
        let mime_type = item.mime_type.as_deref();
        let render: fn(&str) -> Vec<Line<'static>> =
            if markdown::is_markdown_document(file_name, mime_type) {
                markdown::render
            } else if html::is_html_document(file_name, mime_type) {
                html::render
            } else {
                return;
            };
        if !self.ensure_fresh_session().await {
            return;
        }
        let session = self.session.clone().unwrap();
        let token = match self.supabase.issue_download_token(&session, item.id).await {
            Ok(t) => t,
            Err(e) => {
                self.set_status(format!("could not load preview: {e}"));
                return;
            }
        };
        match self.bot.download(&token).await {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                self.detail_body = Some(render(&text));
            }
            Err(e) => self.set_status(format!("could not load preview: {e}")),
        }
    }

    async fn download_selected(&mut self) {
        let Some(item) = self.items.get(self.selected).cloned() else {
            return;
        };
        if item.kind == "note" {
            self.set_status("Notes have no file to download.".to_string());
            return;
        }
        if !self.ensure_fresh_session().await {
            return;
        }
        let session = self.session.clone().unwrap();
        let token = match self.supabase.issue_download_token(&session, item.id).await {
            Ok(t) => t,
            Err(e) => {
                self.set_status(format!("could not download: {e}"));
                return;
            }
        };
        match self.bot.download(&token).await {
            Ok(bytes) => {
                let name = item
                    .file_name
                    .clone()
                    .unwrap_or_else(|| format!("{}.bin", item.short_id()));
                match tokio::fs::write(&name, &bytes).await {
                    Ok(()) => self.set_status(format!("Saved to ./{name}")),
                    Err(e) => self.set_status(format!("could not write {name}: {e}")),
                }
            }
            Err(e) => self.set_status(format!("could not download: {e}")),
        }
    }
}

fn handle_esc_back(code: KeyCode, screen: &mut Screen, back_to: Screen) {
    if matches!(code, KeyCode::Esc | KeyCode::Char('q')) {
        *screen = back_to;
    }
}
