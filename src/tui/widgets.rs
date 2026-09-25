//! Small building blocks: colours, a keyboard form over tui-input fields, popups.

use std::str::FromStr;

use ratatui::{
    Frame,
    crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Clear, Paragraph},
};
use tui_input::{Input, backend::crossterm::EventHandler};
use tui_popup::Popup;

pub const DIM: Color = Color::Rgb(0x9E, 0xAE, 0xBA);
pub const WARN: Color = Color::Rgb(0xFF, 0x8A, 0x80);
pub const OK: Color = Color::Rgb(0x5D, 0xB5, 0x85);
pub const SURFACE: Color = Color::Rgb(0x22, 0x30, 0x39);
pub const LINE: Color = Color::Rgb(0x2E, 0x3E, 0x4A);

/// A `#RRGGBB` library colour, grey when it doesn't parse.
pub fn hex(s: &str) -> Color {
    Color::from_str(s).unwrap_or(Color::Gray)
}

/// A count as terminal cells, saturating at `u16::MAX`.
#[must_use]
pub fn cells(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

pub fn is_press(k: &KeyEvent) -> bool {
    matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

pub enum FieldKind {
    Text(Input),
    Choice { options: Vec<String>, idx: usize },
    Toggle(bool),
}

pub struct Field {
    pub label: &'static str,
    pub kind: FieldKind,
}

impl Field {
    pub fn text(label: &'static str, value: impl Into<String>) -> Field {
        Field {
            label,
            kind: FieldKind::Text(Input::new(value.into())),
        }
    }
    pub fn choice(label: &'static str, options: Vec<String>, idx: usize) -> Field {
        Field {
            label,
            kind: FieldKind::Choice { options, idx },
        }
    }
    pub fn toggle(label: &'static str, on: bool) -> Field {
        Field {
            label,
            kind: FieldKind::Toggle(on),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum FormEvent {
    None,
    Changed(usize),
    Submit,
    Cancel,
}

/// Labelled fields navigated with Tab / Shift-Tab; Ctrl-s or Enter on the last field submits.
pub struct Form {
    pub title: String,
    pub accent: Color,
    pub fields: Vec<Field>,
    pub focus: usize,
    pub error: Option<String>,
}

const LABEL_W: u16 = 16;

impl Form {
    pub fn new(title: impl Into<String>, accent: Color, fields: Vec<Field>) -> Form {
        Form {
            title: title.into(),
            accent,
            fields,
            focus: 0,
            error: None,
        }
    }

    pub fn text(&self, i: usize) -> &str {
        match &self.fields[i].kind {
            FieldKind::Text(input) => input.value().trim(),
            _ => "",
        }
    }
    pub fn set_text(&mut self, i: usize, v: String) {
        if let FieldKind::Text(input) = &mut self.fields[i].kind {
            *input = Input::new(v);
        }
    }
    pub fn choice(&self, i: usize) -> usize {
        match self.fields[i].kind {
            FieldKind::Choice { idx, .. } => idx,
            _ => 0,
        }
    }
    pub fn toggle(&self, i: usize) -> bool {
        matches!(self.fields[i].kind, FieldKind::Toggle(true))
    }

    /// Parse text field `i`, naming the field in the error.
    pub fn parse<T: FromStr>(&self, i: usize) -> Result<T, String> {
        self.text(i)
            .parse()
            .map_err(|_| format!("{}: not a valid number", self.fields[i].label))
    }

    pub fn handle(&mut self, ev: &Event) -> FormEvent {
        let Event::Key(k) = ev else {
            return FormEvent::None;
        };
        if !is_press(k) {
            return FormEvent::None;
        }
        let n = self.fields.len();
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc => return FormEvent::Cancel,
            KeyCode::Char('s') if ctrl => return FormEvent::Submit,
            KeyCode::Enter if self.focus + 1 == n => return FormEvent::Submit,
            KeyCode::Enter | KeyCode::Tab | KeyCode::Down => {
                self.focus = (self.focus + 1) % n;
                return FormEvent::None;
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.focus = (self.focus + n - 1) % n;
                return FormEvent::None;
            }
            _ => {}
        }
        let focus = self.focus;
        match &mut self.fields[focus].kind {
            FieldKind::Text(input) => {
                if input.handle_event(ev).is_some_and(|c| c.value) {
                    return FormEvent::Changed(focus);
                }
            }
            FieldKind::Choice { options, idx } => {
                let len = options.len().max(1);
                match k.code {
                    KeyCode::Left | KeyCode::Char('h') => *idx = (*idx + len - 1) % len,
                    KeyCode::Right | KeyCode::Char('l' | ' ') => {
                        *idx = (*idx + 1) % len;
                    }
                    _ => return FormEvent::None,
                }
                return FormEvent::Changed(focus);
            }
            FieldKind::Toggle(on) => {
                if matches!(
                    k.code,
                    KeyCode::Left | KeyCode::Right | KeyCode::Char(' ' | 'h' | 'l')
                ) {
                    *on = !*on;
                    return FormEvent::Changed(focus);
                }
            }
        }
        FormEvent::None
    }

    /// Rows needed by [`Form::render_fields`].
    pub fn height(&self) -> u16 {
        cells(self.fields.len()) + 2
    }

    /// One row per field, then the error (if any) and the key hint; places the cursor.
    pub fn render_fields(&self, f: &mut Frame, area: Rect) {
        let rows =
            Layout::vertical((0..self.fields.len() + 2).map(|_| Constraint::Length(1))).split(area);
        for (i, field) in self.fields.iter().enumerate() {
            let focused = i == self.focus;
            let [label, value] =
                Layout::horizontal([Constraint::Length(LABEL_W), Constraint::Fill(1)])
                    .areas(rows[i]);
            let label_style = if focused {
                Style::new().fg(self.accent).bold()
            } else {
                Style::new().fg(DIM)
            };
            let marker = if focused { "› " } else { "  " };
            f.render_widget(
                Paragraph::new(format!("{marker}{}", field.label)).style(label_style),
                label,
            );
            let value_style = if focused {
                Style::new().bg(SURFACE)
            } else {
                Style::new()
            };
            match &field.kind {
                FieldKind::Text(input) => {
                    let w = value.width.saturating_sub(1) as usize;
                    let scroll = input.visual_scroll(w);
                    f.render_widget(
                        Paragraph::new(input.value())
                            .style(value_style)
                            .scroll((0, cells(scroll))),
                        value,
                    );
                    if focused {
                        let x = input.visual_cursor().max(scroll) - scroll;
                        f.set_cursor_position((value.x + cells(x), value.y));
                    }
                }
                FieldKind::Choice { options, idx } => {
                    let v = options.get(*idx).map_or("", String::as_str);
                    f.render_widget(Paragraph::new(format!("‹ {v} ›")).style(value_style), value);
                }
                FieldKind::Toggle(on) => {
                    let v = if *on { "[x] yes" } else { "[ ] no" };
                    f.render_widget(Paragraph::new(v).style(value_style), value);
                }
            }
        }
        let n = self.fields.len();
        if let Some(e) = &self.error {
            f.render_widget(Paragraph::new(e.as_str()).fg(WARN), rows[n]);
        }
        f.render_widget(
            Paragraph::new("Tab/Shift-Tab field · ←/→ choose · Ctrl-s save · Esc cancel").fg(DIM),
            rows[n + 1],
        );
    }

    /// The form alone in a centred popup.
    pub fn render_popup(&self, f: &mut Frame) {
        let area = popup_area(f.area(), 64, self.height() + 2);
        let inner = frame_popup(f, area, &self.title, self.accent);
        self.render_fields(f, inner);
    }
}

/// A centred rectangle of at most `w` × `h`.
pub fn popup_area(area: Rect, w: u16, h: u16) -> Rect {
    area.centered(
        Constraint::Length(w.min(area.width)),
        Constraint::Length(h.min(area.height)),
    )
}

/// Clear `area`, draw a titled border and return the inside.
pub fn frame_popup(f: &mut Frame, area: Rect, title: &str, accent: Color) -> Rect {
    let block = Block::bordered()
        .title(Span::styled(
            format!(" {title} "),
            Style::new().fg(accent).bold(),
        ))
        .border_style(Style::new().fg(accent));
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    inner
}

/// A self-sizing message popup.
pub fn message_popup(f: &mut Frame, title: &str, body: Text<'static>, accent: Color) {
    let popup = Popup::new(body)
        .title(Line::from(format!(" {title} ")).bold())
        .border_style(Style::new().fg(accent));
    f.render_widget(popup, f.area());
}
