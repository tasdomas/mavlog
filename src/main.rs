mod tlog;

use std::{env, fs};

use anyhow::{bail, Context, Result};
use chrono::DateTime;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseEventKind};
use ratatui::{
    layout::{Constraint, Flex, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Clear, Paragraph, Row, Table, Wrap},
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

#[derive(PartialEq, Clone, Copy)]
enum TimeFormat {
    DateTime,
    OffsetSecs,
}

struct Settings {
    time_format: TimeFormat,
}

/// One row in the settings menu: a label plus accessors for cycling and
/// showing the current value.
struct SettingItem {
    label: &'static str,
    value: fn(&Settings) -> &'static str,
    toggle: fn(&mut Settings),
}

const SETTING_ITEMS: &[SettingItem] = &[SettingItem {
    label: "Time column",
    value: |s| match s.time_format {
        TimeFormat::DateTime => "date-time",
        TimeFormat::OffsetSecs => "offset (s)",
    },
    toggle: |s| {
        s.time_format = match s.time_format {
            TimeFormat::DateTime => TimeFormat::OffsetSecs,
            TimeFormat::OffsetSecs => TimeFormat::DateTime,
        }
    },
}];

struct App {
    path: String,
    data: Vec<u8>,
    entries: Vec<tlog::LogEntry>,
    /// Timestamp of the first message; offsets are relative to it.
    start_us: u64,
    selected: usize,
    offset: usize,
    view_height: usize,
    focus: Focus,
    detail_scroll: usize,
    settings: Settings,
    settings_open: bool,
    settings_selected: usize,
}

impl App {
    fn new(path: String, data: Vec<u8>, entries: Vec<tlog::LogEntry>) -> Self {
        Self {
            path,
            data,
            start_us: entries[0].timestamp_us,
            entries,
            selected: 0,
            offset: 0,
            view_height: 1,
            focus: Focus::List,
            detail_scroll: 0,
            settings: Settings {
                time_format: TimeFormat::DateTime,
            },
            settings_open: false,
            settings_selected: 0,
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
                    if self.settings_open {
                        continue;
                    }
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
        if self.settings_open {
            self.handle_settings_key(code);
            return true;
        }
        match code {
            KeyCode::Char('q') => return false,
            KeyCode::Char('s') => self.settings_open = true,
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

    fn handle_settings_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') | KeyCode::Char('s') | KeyCode::Esc => {
                self.settings_open = false
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.settings_selected = self.settings_selected.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.settings_selected =
                    (self.settings_selected + 1).min(SETTING_ITEMS.len() - 1)
            }
            KeyCode::Enter
            | KeyCode::Char(' ')
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Char('h')
            | KeyCode::Char('l') => {
                (SETTING_ITEMS[self.settings_selected].toggle)(&mut self.settings)
            }
            _ => {}
        }
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
        let hints = if self.settings_open {
            " ↑/↓ select  Enter/Space toggle  Esc close"
        } else {
            match self.focus {
                Focus::List => " ↑/↓ select  PgUp/PgDn  g/G top/bottom  →/Tab details  s settings  q quit",
                Focus::Detail => " ↑/↓ scroll  PgUp/PgDn  ←/Esc back to list  s settings  q quit",
            }
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

        if self.settings_open {
            self.draw_settings(frame);
        }
    }

    fn draw_settings(&self, frame: &mut Frame) {
        let width = 40;
        let height = SETTING_ITEMS.len() as u16 + 2;
        let [area] = Layout::vertical([Constraint::Length(height)])
            .flex(Flex::Center)
            .areas(frame.area());
        let [area] = Layout::horizontal([Constraint::Length(width)])
            .flex(Flex::Center)
            .areas(area);

        let inner_width = (width as usize).saturating_sub(2);
        let lines: Vec<Line> = SETTING_ITEMS
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let value = format!("◂ {} ▸", (item.value)(&self.settings));
                let text = format!(
                    " {}{value:>vw$} ",
                    item.label,
                    vw = inner_width.saturating_sub(item.label.len() + 2),
                );
                if i == self.settings_selected {
                    Line::styled(text, Style::new().add_modifier(Modifier::REVERSED))
                } else {
                    Line::raw(text)
                }
            })
            .collect();

        let block = Block::bordered()
            .title(Line::styled(
                " Settings ",
                Style::new().add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::new().fg(Color::Cyan));
        frame.render_widget(Clear, area);
        frame.render_widget(Paragraph::new(lines).block(block), area);
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
        let time_width = match self.settings.time_format {
            TimeFormat::DateTime => 23,
            TimeFormat::OffsetSecs => 12,
        };
        let rows = self.entries[self.offset..end]
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let index = self.offset + i;
                let row = Row::new(vec![
                    format!("{index:>7}"),
                    self.format_list_time(entry.timestamp_us),
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
                Constraint::Length(time_width),
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
        let mut body = format!(
            "Time: {}  ({})\n\n",
            format_datetime(entry.timestamp_us),
            format_offset(entry.timestamp_us, self.start_us),
        );
        body.push_str(&match tlog::decode(&self.data, entry) {
            Ok(msg) => format!("{msg:#?}"),
            Err(_) => hex_dump(&self.data[entry.payload.clone()]),
        });
        let lines: Vec<&str> = body.lines().collect();

        let inner_height = (area.height as usize).saturating_sub(2);
        self.detail_scroll = self
            .detail_scroll
            .min(lines.len().saturating_sub(inner_height));

        let title = format!(" {} (id {}) ", entry.name, entry.msg_id);
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
        let color = if self.focus == pane && !self.settings_open {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        Block::bordered().border_style(Style::new().fg(color))
    }

    fn format_list_time(&self, timestamp_us: u64) -> String {
        match self.settings.time_format {
            TimeFormat::DateTime => format_datetime(timestamp_us),
            TimeFormat::OffsetSecs => format_offset(timestamp_us, self.start_us),
        }
    }
}

/// Seconds relative to the start of the log, e.g. "T+4.600s".
fn format_offset(timestamp_us: u64, start_us: u64) -> String {
    let secs = (timestamp_us as i64 - start_us as i64) as f64 / 1e6;
    format!("T{secs:+.3}s")
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
