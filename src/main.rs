mod tlog;

use std::{env, fs};

use anyhow::{bail, Context, Result};
use chrono::DateTime;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseEventKind};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Row, Table},
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
    let result = App::new(path, entries).run(&mut terminal);
    let _ = crossterm::execute!(std::io::stdout(), event::DisableMouseCapture);
    ratatui::restore();
    result
}

struct App {
    path: String,
    entries: Vec<tlog::LogEntry>,
    selected: usize,
    offset: usize,
    view_height: usize,
}

impl App {
    fn new(path: String, entries: Vec<tlog::LogEntry>) -> Self {
        Self {
            path,
            entries,
            selected: 0,
            offset: 0,
            view_height: 1,
        }
    }

    fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        loop {
            terminal.draw(|frame| self.draw(frame))?;
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
                    KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
                    KeyCode::PageUp => self.move_by(-(self.view_height as isize)),
                    KeyCode::PageDown => self.move_by(self.view_height as isize),
                    KeyCode::Home | KeyCode::Char('g') => self.selected = 0,
                    KeyCode::End | KeyCode::Char('G') => {
                        self.selected = self.entries.len() - 1
                    }
                    _ => {}
                },
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => self.move_by(-3),
                    MouseEventKind::ScrollDown => self.move_by(3),
                    _ => {}
                },
                _ => {}
            }
        }
    }

    fn move_by(&mut self, delta: isize) {
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.entries.len() - 1);
    }

    fn draw(&mut self, frame: &mut Frame) {
        let [header_area, list_area, footer_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        // The table area loses two rows to the top border and column header.
        self.view_height = (list_area.height as usize).saturating_sub(2).max(1);

        // Keep the selection inside the visible window.
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + self.view_height {
            self.offset = self.selected - self.view_height + 1;
        }

        let bar_style = Style::new().fg(Color::Black).bg(Color::Cyan);
        frame.render_widget(
            Line::styled(
                format!(" {}  —  {} messages", self.path, self.entries.len()),
                bar_style.add_modifier(Modifier::BOLD),
            ),
            header_area,
        );

        let end = (self.offset + self.view_height).min(self.entries.len());
        let rows = self.entries[self.offset..end]
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let index = self.offset + i;
                let row = Row::new(vec![
                    format!("{index:>8}"),
                    format_time(entry.timestamp_us),
                    format!("{:>3}:{:<3}", entry.sysid, entry.compid),
                    entry.name.clone(),
                ]);
                if index == self.selected {
                    row.style(Style::new().add_modifier(Modifier::REVERSED))
                } else {
                    row
                }
            });

        let table = Table::new(
            rows,
            [
                Constraint::Length(8),
                Constraint::Length(23),
                Constraint::Length(7),
                Constraint::Fill(1),
            ],
        )
        .header(
            Row::new(vec!["#", "TIME", "SYS:CMP", "MESSAGE"])
                .style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        )
        .block(Block::new().borders(Borders::TOP));
        frame.render_widget(table, list_area);

        let position = format!("{}/{} ", self.selected + 1, self.entries.len());
        let hints = " ↑/↓ scroll  PgUp/PgDn page  g/G top/bottom  q quit";
        frame.render_widget(
            Line::styled(
                format!(
                    "{hints}{position:>width$}",
                    width = (footer_area.width as usize).saturating_sub(hints.len()),
                ),
                bar_style,
            ),
            footer_area,
        );
    }
}

fn format_time(timestamp_us: u64) -> String {
    match DateTime::from_timestamp_micros(timestamp_us as i64) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        None => format!("({timestamp_us} us)"),
    }
}
