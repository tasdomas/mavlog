mod tlog;

use std::{env, fs};

use anyhow::{bail, Context, Result};
use chrono::DateTime;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseEventKind};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Paragraph, Row, Table, Wrap},
    DefaultTerminal, Frame,
};

fn main() -> Result<()> {
    let path = env::args()
        .nth(1)
        .context("usage: mavlog <file.tlog>")?;
    let data = fs::read(&path).with_context(|| format!("failed to read {path}"))?;
    let entries = tlog::parse(&data);
    if entries.is_empty() {
        bail!("no MAVLink messages found in {path}");
    }

    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), event::EnableMouseCapture);
    let result = App::new(path, data, entries).run(&mut terminal);
    let _ = crossterm::execute!(std::io::stdout(), event::DisableMouseCapture);
    ratatui::restore();
    result
}

#[derive(PartialEq, Clone, Copy)]
enum Focus {
    List,
    Detail,
}

struct App {
    path: String,
    data: Vec<u8>,
    entries: Vec<tlog::LogEntry>,
    selected: usize,
    offset: usize,
    view_height: usize,
    focus: Focus,
    detail_scroll: usize,
}

impl App {
    fn new(path: String, data: Vec<u8>, entries: Vec<tlog::LogEntry>) -> Self {
        Self {
            path,
            data,
            entries,
            selected: 0,
            offset: 0,
            view_height: 1,
            focus: Focus::List,
            detail_scroll: 0,
        }
    }

    fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        loop {
            terminal.draw(|frame| self.draw(frame))?;
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if !self.handle_key(key.code) {
                        return Ok(());
                    }
                }
                Event::Mouse(mouse) => {
                    let delta = match mouse.kind {
                        MouseEventKind::ScrollUp => -3,
                        MouseEventKind::ScrollDown => 3,
                        _ => continue,
                    };
                    self.scroll_by(delta);
                }
                _ => {}
            }
        }
    }

    /// Returns false when the app should quit.
    fn handle_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Char('q') => return false,
            KeyCode::Esc => {
                if self.focus == Focus::Detail {
                    self.focus = Focus::List;
                } else {
                    return false;
                }
            }
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Enter => {
                self.focus = match self.focus {
                    Focus::List => Focus::Detail,
                    Focus::Detail => Focus::List,
                };
            }
            KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::List,
            KeyCode::Right | KeyCode::Char('l') => self.focus = Focus::Detail,
            KeyCode::Up | KeyCode::Char('k') => self.scroll_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_by(1),
            KeyCode::PageUp => self.scroll_by(-(self.view_height as isize)),
            KeyCode::PageDown => self.scroll_by(self.view_height as isize),
            KeyCode::Home | KeyCode::Char('g') => match self.focus {
                Focus::List => self.select(0),
                Focus::Detail => self.detail_scroll = 0,
            },
            KeyCode::End | KeyCode::Char('G') => match self.focus {
                Focus::List => self.select(self.entries.len() - 1),
                Focus::Detail => self.detail_scroll = usize::MAX, // clamped in draw
            },
            _ => {}
        }
        true
    }

    /// Scroll whichever pane has focus.
    fn scroll_by(&mut self, delta: isize) {
        match self.focus {
            Focus::List => self.select(
                self.selected
                    .saturating_add_signed(delta)
                    .min(self.entries.len() - 1),
            ),
            Focus::Detail => {
                self.detail_scroll = self.detail_scroll.saturating_add_signed(delta);
            }
        }
    }

    fn select(&mut self, index: usize) {
        if index != self.selected {
            self.selected = index;
            self.detail_scroll = 0;
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let [header_area, main_area, footer_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        let [list_area, detail_area] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                .areas(main_area);

        let bar_style = Style::new().fg(Color::Black).bg(Color::Cyan);
        frame.render_widget(
            Line::styled(
                format!(" {}  —  {} messages", self.path, self.entries.len()),
                bar_style.add_modifier(Modifier::BOLD),
            ),
            header_area,
        );

        self.draw_list(frame, list_area);
        self.draw_detail(frame, detail_area);

        let position = format!("{}/{} ", self.selected + 1, self.entries.len());
        let hints = match self.focus {
            Focus::List => " ↑/↓ select  PgUp/PgDn  g/G top/bottom  →/Tab details  q quit",
            Focus::Detail => " ↑/↓ scroll  PgUp/PgDn  g/G top/bottom  ←/Esc back to list  q quit",
        };
        frame.render_widget(
            Line::styled(
                format!(
                    "{hints}{position:>width$}",
                    width = (footer_area.width as usize)
                        .saturating_sub(hints.chars().count()),
                ),
                bar_style,
            ),
            footer_area,
        );
    }

    fn draw_list(&mut self, frame: &mut Frame, area: ratatui::layout::Rect) {
        // Two border rows plus the column header row.
        self.view_height = (area.height as usize).saturating_sub(3).max(1);

        // Keep the selection inside the visible window.
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + self.view_height {
            self.offset = self.selected - self.view_height + 1;
        }

        let end = (self.offset + self.view_height).min(self.entries.len());
        let selection_style = if self.focus == Focus::List {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new().fg(Color::Black).bg(Color::DarkGray)
        };
        let rows = self.entries[self.offset..end]
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let index = self.offset + i;
                let row = Row::new(vec![
                    format!("{index:>7}"),
                    format_time(entry.timestamp_us),
                    format!("{:>3}:{:<3}", entry.sysid, entry.compid),
                    entry.name.clone(),
                ]);
                if index == self.selected {
                    row.style(selection_style)
                } else {
                    row
                }
            });

        let table = Table::new(
            rows,
            [
                Constraint::Length(7),
                Constraint::Length(12),
                Constraint::Length(7),
                Constraint::Fill(1),
            ],
        )
        .header(
            Row::new(vec!["#", "TIME", "SYS:CMP", "MESSAGE"])
                .style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        )
        .block(self.pane_block(Focus::List).title(" Messages "));
        frame.render_widget(table, area);
    }

    fn draw_detail(&mut self, frame: &mut Frame, area: ratatui::layout::Rect) {
        let entry = &self.entries[self.selected];
        let body = match tlog::decode(&self.data, entry) {
            Ok(msg) => format!("{msg:#?}"),
            Err(_) => hex_dump(&self.data[entry.payload.clone()]),
        };
        let lines: Vec<&str> = body.lines().collect();

        let inner_height = (area.height as usize).saturating_sub(2);
        self.detail_scroll = self
            .detail_scroll
            .min(lines.len().saturating_sub(inner_height));

        let title = format!(
            " {} (id {})  {} ",
            entry.name,
            entry.msg_id,
            format_datetime(entry.timestamp_us),
        );
        let mut block = self
            .pane_block(Focus::Detail)
            .title(Line::styled(title, Style::new().add_modifier(Modifier::BOLD)));
        if lines.len() > inner_height {
            block = block.title_bottom(
                Line::raw(format!(
                    " {}-{}/{} ",
                    self.detail_scroll + 1,
                    (self.detail_scroll + inner_height).min(lines.len()),
                    lines.len(),
                ))
                .right_aligned(),
            );
        }

        let text: Vec<Line> = lines
            .iter()
            .skip(self.detail_scroll)
            .take(inner_height)
            .map(|line| Line::raw(*line))
            .collect();
        frame.render_widget(
            Paragraph::new(text).wrap(Wrap { trim: false }).block(block),
            area,
        );
    }

    fn pane_block(&self, pane: Focus) -> Block<'static> {
        let color = if self.focus == pane {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        Block::bordered().border_style(Style::new().fg(color))
    }
}

fn format_time(timestamp_us: u64) -> String {
    match DateTime::from_timestamp_micros(timestamp_us as i64) {
        Some(dt) => dt.format("%H:%M:%S%.3f").to_string(),
        None => format!("({timestamp_us} us)"),
    }
}

fn format_datetime(timestamp_us: u64) -> String {
    match DateTime::from_timestamp_micros(timestamp_us as i64) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        None => format!("({timestamp_us} us)"),
    }
}

/// Fallback body for messages the dialect can't decode.
fn hex_dump(payload: &[u8]) -> String {
    let mut out = String::from("undecodable payload:\n");
    for (i, chunk) in payload.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        let ascii: String = chunk
            .iter()
            .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
            .collect();
        out.push_str(&format!("{:04x}  {:<47}  {ascii}\n", i * 16, hex.join(" ")));
    }
    out
}
