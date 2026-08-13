use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, EditFocus, LoginStage, Screen, UploadFocus, UPLOAD_KINDS};

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(f.area());

    match app.screen {
        Screen::Login => draw_login(f, app, chunks[0]),
        Screen::List => draw_list(f, app, chunks[0]),
        Screen::Detail => draw_detail(f, app, chunks[0]),
        Screen::Search => draw_search(f, app, chunks[0]),
        Screen::AddNote => draw_add_note(f, app, chunks[0]),
        Screen::UploadFile => draw_upload(f, app, chunks[0]),
        Screen::EditItem => draw_edit(f, app, chunks[0]),
        Screen::Stats => draw_stats(f, app, chunks[0]),
    }

    draw_status(f, app, chunks[1]);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let text = app.status.clone().unwrap_or_else(|| hint_for(&app.screen));
    f.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn hint_for(screen: &Screen) -> String {
    match screen {
        Screen::Login => "Enter email, Enter again for the code from that email. Esc to quit.".to_string(),
        Screen::List => "j/k move  Enter view  / search  n note  u upload  e edit  x delete  d download  t stats  r refresh  q quit".to_string(),
        Screen::Detail => "Esc back".to_string(),
        Screen::Search => "Type a query, Enter to search, Esc to cancel".to_string(),
        Screen::AddNote => "Type a note, Enter to save, Esc to cancel".to_string(),
        Screen::UploadFile => "Tab to move field, Left/Right to change kind, Enter to upload, Esc to cancel".to_string(),
        Screen::EditItem => "Tab to move field, Enter to save, Esc to cancel".to_string(),
        Screen::Stats => "Esc back".to_string(),
    }
}

fn draw_login(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title("telenotes login")
        .borders(Borders::ALL);
    let text = match app.login_stage {
        LoginStage::Email => format!("Email: {}_", app.login_email),
        LoginStage::Code => format!("Email: {}\nCode: {}_", app.login_email, app.login_code),
    };
    f.render_widget(Paragraph::new(text).block(block), area);
}

fn draw_list(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = if app.items.is_empty() {
        vec![ListItem::new("Nothing yet -- press n to add a note.")]
    } else {
        app.items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let style = if i == app.selected {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                ListItem::new(item.summary()).style(style)
            })
            .collect()
    };
    let block = Block::default()
        .title(format!("telenotes  ({} items)", app.items.len()))
        .borders(Borders::ALL);
    f.render_widget(List::new(items).block(block), area);
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let Some(item) = app.items.get(app.selected) else {
        f.render_widget(Paragraph::new("Nothing selected."), area);
        return;
    };
    let mut lines = vec![
        Line::from(format!("id: {}", item.short_id())),
        Line::from(format!("kind: {}", item.kind)),
        Line::from(format!("created: {}", item.created_at.to_rfc3339())),
        Line::from(format!("tags: {}", item.tags.join(", "))),
        Line::from(""),
    ];
    if let Some(text) = &item.content_text {
        lines.push(Line::from(text.as_str()));
    }
    if let Some(name) = &item.file_name {
        lines.push(Line::from(format!("file: {name}")));
    }
    if let Some(body) = &app.detail_body {
        lines.push(Line::from(""));
        lines.extend(body.iter().cloned());
    }
    let block = Block::default().title("Detail").borders(Borders::ALL);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block),
        area,
    );
}

fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default().title("Search").borders(Borders::ALL);
    f.render_widget(
        Paragraph::new(format!("{}_", app.search_query)).block(block),
        area,
    );
}

fn draw_add_note(f: &mut Frame, app: &App, area: Rect) {
    let tags_preview = telenotes_core::text::extract_tags(&app.note_text);
    let preview = if tags_preview.is_empty() {
        String::new()
    } else {
        format!("\n\ntags: {}", tags_preview.join(", "))
    };
    let block = Block::default().title("New note").borders(Borders::ALL);
    f.render_widget(
        Paragraph::new(format!("{}_{preview}", app.note_text))
            .wrap(Wrap { trim: false })
            .block(block),
        area,
    );
}

fn field(label: &str, value: &str, focused: bool) -> Paragraph<'static> {
    let style = if focused {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };
    Paragraph::new(format!("{value}_"))
        .style(style)
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title(label.to_string())
                .borders(Borders::ALL),
        )
}

fn draw_upload(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(area);

    f.render_widget(
        field(
            "Path",
            &app.upload_path,
            app.upload_focus == UploadFocus::Path,
        ),
        chunks[0],
    );
    f.render_widget(
        field(
            "Kind (Left/Right)",
            UPLOAD_KINDS[app.upload_kind_idx],
            app.upload_focus == UploadFocus::Kind,
        ),
        chunks[1],
    );
    f.render_widget(
        field(
            "Caption",
            &app.upload_caption,
            app.upload_focus == UploadFocus::Caption,
        ),
        chunks[2],
    );
}

fn draw_edit(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(area);

    f.render_widget(
        field("Text", &app.edit_text, app.edit_focus == EditFocus::Text),
        chunks[0],
    );
    f.render_widget(
        field(
            "Tags (comma-separated)",
            &app.edit_tags,
            app.edit_focus == EditFocus::Tags,
        ),
        chunks[1],
    );
    f.render_widget(
        field(
            "File name",
            &app.edit_file_name,
            app.edit_focus == EditFocus::FileName,
        ),
        chunks[2],
    );
    f.render_widget(
        field(
            "Mime type",
            &app.edit_mime_type,
            app.edit_focus == EditFocus::MimeType,
        ),
        chunks[3],
    );
}

fn draw_stats(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(area);

    let tag_lines: Vec<Line> = app
        .tags
        .iter()
        .map(|t| Line::from(format!("#{} ({})", t.tag, t.n)))
        .collect();
    f.render_widget(
        Paragraph::new(tag_lines).block(Block::default().title("Tags").borders(Borders::ALL)),
        chunks[0],
    );

    let max_n = app.days.iter().map(|d| d.n).max().unwrap_or(1).max(1);
    let day_lines: Vec<Line> = app
        .days
        .iter()
        .map(|d| {
            let bar_len = ((d.n as f64 / max_n as f64) * 20.0) as usize;
            Line::from(format!("{} {} {}", d.day, "#".repeat(bar_len), d.n))
        })
        .collect();
    f.render_widget(
        Paragraph::new(day_lines)
            .block(Block::default().title("Last 30 days").borders(Borders::ALL)),
        chunks[1],
    );

    let storage_lines: Vec<Line> = app
        .storage
        .iter()
        .map(|s| {
            Line::from(format!(
                "{}/{}: {} ({:.1} MB)",
                s.kind,
                s.backend,
                s.n,
                s.bytes as f64 / 1024.0 / 1024.0
            ))
        })
        .collect();
    f.render_widget(
        Paragraph::new(storage_lines)
            .block(Block::default().title("Storage").borders(Borders::ALL)),
        chunks[2],
    );
}
