//! High-contrast, low-power terminal interface for field use.
//!
//! Two restrained palettes (red-adapted and green-adapted) keep the screen
//! readable at night without destroying dark adaptation. No animation, no
//! full-screen repaint beyond the diff ratatui already does.

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::{execute, terminal};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{Frame, Terminal};
use std::io::{self, Stdout};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub name: &'static str,
    pub bg: Color,
    pub fg: Color,
    pub dim: Color,
    pub accent: Color,
    pub user: Color,
}

pub const THEMES: &[Theme] = &[
    Theme {
        name: "red",
        bg: Color::Rgb(8, 6, 6),
        fg: Color::Rgb(255, 92, 72),
        dim: Color::Rgb(122, 46, 38),
        accent: Color::Rgb(255, 176, 64),
        user: Color::Rgb(210, 210, 210),
    },
    Theme {
        name: "green",
        bg: Color::Rgb(4, 8, 6),
        fg: Color::Rgb(96, 255, 148),
        dim: Color::Rgb(34, 96, 58),
        accent: Color::Rgb(196, 255, 96),
        user: Color::Rgb(210, 210, 210),
    },
    Theme {
        name: "amber",
        bg: Color::Rgb(8, 7, 4),
        fg: Color::Rgb(255, 190, 92),
        dim: Color::Rgb(112, 78, 32),
        accent: Color::Rgb(255, 236, 160),
        user: Color::Rgb(214, 214, 214),
    },
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    System,
}

pub struct Turn {
    pub role: Role,
    pub text: String,
}

/// Messages sent from the inference task to the UI loop.
pub enum StreamMsg {
    Delta(String),
    Final(String),
    Error(String),
}

pub enum Action {
    None,
    Submit(String),
    Cancel,
    Quit,
}

pub struct App {
    pub theme: Theme,
    pub input: String,
    pub turns: Vec<Turn>,
    pub busy: bool,
    pub follow: bool,
    pub scroll: u16,
    pub sources: Vec<String>,
    pub model: String,
    pub retrieval_only: bool,
    pub quit: bool,
}

impl App {
    pub fn new(model: String, retrieval_only: bool) -> Self {
        let mut turns = Vec::new();
        turns.push(Turn {
            role: Role::System,
            text: if retrieval_only {
                "No GGUF model loaded. Answers are extracted directly from the embedded corpus."
                    .to_string()
            } else {
                "Local model ready. Nothing leaves this device.".to_string()
            },
        });
        Self {
            theme: THEMES[0],
            input: String::new(),
            turns,
            busy: false,
            follow: true,
            scroll: 0,
            sources: Vec::new(),
            model,
            retrieval_only,
            quit: false,
        }
    }

    pub fn push(&mut self, role: Role, text: impl Into<String>) {
        self.turns.push(Turn {
            role,
            text: text.into(),
        });
        self.follow = true;
    }

    pub fn delta(&mut self, text: &str) {
        if let Some(last) = self.turns.last_mut() {
            if last.role == Role::Assistant {
                last.text.push_str(text);
                return;
            }
        }
        self.push(Role::Assistant, text);
    }

    pub fn finalize(&mut self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if let Some(last) = self.turns.last_mut() {
            if last.role == Role::Assistant {
                last.text = text.to_string();
                return;
            }
        }
        self.push(Role::Assistant, text);
    }

    fn cycle_theme(&mut self) {
        let i = THEMES
            .iter()
            .position(|t| t.name == self.theme.name)
            .unwrap_or(0);
        self.theme = THEMES[(i + 1) % THEMES.len()];
    }
}

pub struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl Tui {
    pub fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, terminal::EnterAlternateScreen)?;
        let terminal = Terminal::new(CrosstermBackend::new(out))?;
        Ok(Self { terminal })
    }

    pub fn exit(&mut self) -> io::Result<()> {
        terminal::disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), terminal::LeaveAlternateScreen)?;
        self.terminal.show_cursor()
    }

    pub fn draw(&mut self, app: &mut App) -> io::Result<()> {
        self.terminal.draw(|f| render(f, app))?;
        Ok(())
    }
}

/// Block until a key arrives, or `timeout` expires.
pub fn poll_key(timeout: Duration) -> io::Result<Option<KeyEvent>> {
    if !event::poll(timeout)? {
        return Ok(None);
    }
    match event::read()? {
        Event::Key(key) => Ok(Some(key)),
        _ => Ok(None),
    }
}

pub fn handle_key(app: &mut App, key: KeyEvent) -> Action {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    if ctrl && key.code == KeyCode::Char('c') {
        return Action::Quit;
    }

    match key.code {
        KeyCode::Enter => {
            let text = app.input.trim().to_string();
            if text.is_empty() || app.busy {
                return Action::None;
            }
            app.input.clear();
            Action::Submit(text)
        }
        KeyCode::Char('l') if ctrl => {
            app.turns.clear();
            app.sources.clear();
            Action::None
        }
        KeyCode::Char(c) if !ctrl => {
            if app.input.len() < 2048 {
                app.input.push(c);
            }
            Action::None
        }
        KeyCode::Backspace => {
            app.input.pop();
            Action::None
        }
        KeyCode::Esc => {
            if app.busy {
                app.input.clear();
                Action::Cancel
            } else {
                app.input.clear();
                Action::None
            }
        }
        KeyCode::Tab => {
            app.cycle_theme();
            Action::None
        }
        KeyCode::PageUp => {
            app.follow = false;
            app.scroll = app.scroll.saturating_sub(8);
            Action::None
        }
        KeyCode::PageDown => {
            app.scroll = app.scroll.saturating_add(8);
            Action::None
        }
        KeyCode::Home => {
            app.follow = false;
            app.scroll = 0;
            Action::None
        }
        KeyCode::End => {
            app.follow = true;
            Action::None
        }
        _ => Action::None,
    }
}

fn render(f: &mut Frame, app: &mut App) {
    let t = app.theme;
    let area = f.size();
    f.render_widget(
        Block::default().style(Style::default().bg(t.bg).fg(t.fg)),
        area,
    );

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(f, app, rows[0]);

    // ---- transcript ----
    let body_w = rows[1].width.saturating_sub(4) as usize;
    let mut lines: Vec<Line> = Vec::new();
    for turn in &app.turns {
        let (label, style) = match turn.role {
            Role::User => ("YOU", Style::default().fg(t.user).add_modifier(Modifier::BOLD)),
            Role::Assistant => ("FIELD-EVAL", Style::default().fg(t.fg).add_modifier(Modifier::BOLD)),
            Role::System => ("SYS", Style::default().fg(t.dim)),
        };
        lines.push(Line::from(vec![
            Span::styled(label, style),
            Span::styled(
                match turn.role {
                    Role::User => "",
                    Role::Assistant => "  \u{2500}\u{2500}",
                    Role::System => "  \u{00b7}",
                },
                Style::default().fg(t.dim),
            ),
        ]));
        for wrapped in wrap(&turn.text, body_w.max(20)) {
            lines.push(Line::from(Span::styled(
                wrapped,
                match turn.role {
                    Role::User => Style::default().fg(t.user),
                    Role::Assistant => Style::default().fg(t.fg),
                    Role::System => Style::default().fg(t.dim),
                },
            )));
        }
        lines.push(Line::from(""));
    }

    let view_h = rows[1].height.saturating_sub(2) as usize;
    let total = lines.len();
    let max_scroll = total.saturating_sub(view_h) as u16;
    if app.follow {
        app.scroll = max_scroll;
    } else {
        app.scroll = app.scroll.min(max_scroll);
    }

    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(t.dim))
                    .title(Span::styled(
                        if app.follow { " LOG " } else { " LOG [scrolled] " },
                        Style::default().fg(t.dim),
                    )),
            )
            .scroll((app.scroll, 0)),
        rows[1],
    );

    // ---- input ----
    let prompt = if app.busy { "\u{2026} " } else { "> " };
    let input = Paragraph::new(Line::from(vec![
        Span::styled(prompt, Style::default().fg(t.accent)),
        Span::raw(app.input.as_str()),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if app.busy { t.dim } else { t.accent }))
            .title(Span::styled(" QUERY ", Style::default().fg(t.accent))),
    );
    f.render_widget(input, rows[2]);

    let cx = rows[2].x + 1 + prompt.chars().count() as u16 + app.input.chars().count() as u16;
    let cy = rows[2].y + 1;
    if cx < rows[2].right().saturating_sub(1) {
        f.set_cursor(cx, cy);
    }

    // ---- footer ----
    let ctx = if app.sources.is_empty() {
        "no context".to_string()
    } else {
        app.sources.join(" | ")
    };
    let footer = Line::from(vec![
        Span::styled(
            format!(" {} ", if app.sources.is_empty() { "\u{25cb}" } else { "\u{25cf}" }),
            Style::default().fg(t.accent),
        ),
        Span::styled(truncate(&ctx, area.width as usize / 2), Style::default().fg(t.dim)),
        Span::raw("  "),
        Span::styled(
            "enter ask  tab theme  pgup/dn scroll  ^l clear  ^c quit",
            Style::default().fg(t.dim),
        ),
    ]);
    f.render_widget(Paragraph::new(footer).alignment(Alignment::Left), rows[3]);
}

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let t = app.theme;
    let status = if app.busy {
        Span::styled(" \u{25cf} GENERATING ", Style::default().fg(Color::Black).bg(t.accent))
    } else if app.retrieval_only {
        Span::styled(" \u{25cf} RETRIEVAL ", Style::default().fg(Color::Black).bg(t.dim))
    } else {
        Span::styled(" \u{25cf} ONLINE ", Style::default().fg(Color::Black).bg(t.fg))
    };

    let line = Line::from(vec![
        Span::styled(
            " FIELD-EVAL ",
            Style::default()
                .fg(Color::Black)
                .bg(t.fg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" offline tactical reference ", Style::default().fg(t.dim)),
        status,
        Span::styled(
            format!("  model: {}  \u{2502}  theme: {} ", app.model, t.name),
            Style::default().fg(t.dim),
        ),
    ]);

    f.render_widget(
        Paragraph::new(line)
            .block(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .border_style(Style::default().fg(t.dim)),
            )
            .alignment(Alignment::Left),
        area,
    );
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}\u{2026}")
}

/// Character-width wrapping that preserves explicit newlines.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        for word in paragraph.split(' ') {
            let wlen = word.chars().count();
            if !line.is_empty() && line.chars().count() + 1 + wlen > width {
                out.push(std::mem::take(&mut line));
            }
            if wlen > width {
                // Hard-break pathological long tokens (paths, hashes).
                let mut rest: Vec<char> = word.chars().collect();
                while rest.len() > width {
                    if !line.is_empty() {
                        out.push(std::mem::take(&mut line));
                    }
                    out.push(rest[..width].iter().collect());
                    rest.drain(..width);
                }
                line = rest.iter().collect();
                continue;
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push(line);
    }
    out
}
