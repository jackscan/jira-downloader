use crossterm::event::KeyEventKind;
use futures::StreamExt;
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Rect},
    style::{Modifier, Style},
    widgets::TableState,
};
use std::path::PathBuf;
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Default)]
pub struct App {
    table_state: TableState,
    folder: PathBuf,
    attachments: Vec<Attachment>,
    lengths: (usize, usize, usize, usize),
    exit: bool,
}

#[derive(Debug, Clone)]
struct Attachment {
    filename: String,
    size: usize,
    created: String,
    state: AttachmentState,
    content: String,
}

#[derive(Debug, Clone)]
pub enum AttachmentState {
    NotDownloaded,
    Queued,
    Downloading,
    Downloaded,
}

impl App {
    pub fn new(folder: PathBuf, attachments: Vec<crate::jira::Attachment>) -> Self {
        let attachments: Vec<Attachment> = attachments.into_iter().map(Attachment::from).collect();

        let max_filename_width = attachments
            .iter()
            .map(|att| att.filename.width())
            .max()
            .unwrap_or(0);

        let max_size_width = attachments
            .iter()
            .map(|att| att.size.to_string().width())
            .max()
            .unwrap_or(0);

        let max_created_width = attachments
            .iter()
            .map(|att| att.created.width())
            .max()
            .unwrap_or(0);

        let lengths = (
            3, // State column width
            max_filename_width,
            max_size_width,
            max_created_width,
        );

        Self {
            table_state: TableState::default(),
            folder,
            attachments,
            lengths,
            exit: false,
        }
    }

    pub async fn run(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        while !self.exit {
            let mut evt_reader = crossterm::event::EventStream::new();
            let mut tick = tokio::time::interval(Duration::from_millis(100));

            tokio::select! {
                _ = tick.tick() => {
                    terminal.draw(|frame| {
                        self.draw(frame);
                    })?;
                }
                maybe_evt = evt_reader.next() => {
                    match maybe_evt {
                        Some(Ok(evt)) => match evt {
                            crossterm::event::Event::Key(key_evt)
                                if key_evt.kind == KeyEventKind::Press =>
                            {
                                self.handle_key_press(key_evt);
                            }
                            _ => {}
                        },
                        Some(Err(e)) => {
                            return Err(anyhow::anyhow!("Error reading event: {}", e));
                        }
                        None => {}
                    }
                }
            }
        }

        Ok(())
    }

    fn handle_key_press(&mut self, key_evt: crossterm::event::KeyEvent) {
        match key_evt.code {
            crossterm::event::KeyCode::Char('q') => {
                self.exit = true;
            }
            crossterm::event::KeyCode::Up => {
                self.previous_row();
            }
            crossterm::event::KeyCode::Down => {
                self.next_row();
            }
            _ => {}
        }
    }

    fn next_row(&mut self) {
        self.table_state.select(Some(
            self.table_state
                .selected()
                .map(|i| std::cmp::min(i + 1, self.attachments.len() - 1))
                .unwrap_or(0),
        ));
    }

    fn previous_row(&mut self) {
        self.table_state.select(Some(
            self.table_state
                .selected()
                .map(|i| i.saturating_sub(1))
                .unwrap_or(0),
        ));
    }

    fn draw(&mut self, frame: &mut Frame) {
        self.render_table(frame, frame.area());
    }

    fn render_table(&mut self, frame: &mut Frame, area: Rect) {
        let rows = self.attachments.iter().map(|att| {
            ratatui::widgets::Row::new(vec![
                att.state.to_string(),
                att.filename.clone(),
                att.size.to_string(),
                att.created.clone(),
            ])
        });

        let selected_row_style = Style::default().add_modifier(Modifier::REVERSED);

        let t = ratatui::widgets::Table::new(
            rows,
            [
                Constraint::Max(self.lengths.0 as u16),
                Constraint::Max(self.lengths.1 as u16 + 1),
                Constraint::Max(self.lengths.2 as u16 + 1),
                Constraint::Max(self.lengths.3 as u16),
            ],
        )
        .header(
            ratatui::widgets::Row::new(vec!["", "Filename", "Size", "Created"])
                .style(ratatui::style::Style::default().fg(ratatui::style::Color::Yellow))
                .bottom_margin(1),
        )
        .block(
            ratatui::widgets::Block::default()
            .title("Attachments")
            .borders(ratatui::widgets::Borders::ALL)
        )
        .row_highlight_style(selected_row_style);

        frame.render_stateful_widget(t, area, &mut self.table_state);
    }
}

impl From<crate::jira::Attachment> for Attachment {
    fn from(att: crate::jira::Attachment) -> Self {
        Self {
            filename: att.filename,
            size: att.size as usize,
            created: chrono::DateTime::parse_from_rfc3339(&att.created)
                .map(|dt| {
                    dt.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M")
                        .to_string()
                })
                .unwrap_or_else(|_| att.created.clone()),
            content: att.content,
            state: AttachmentState::NotDownloaded,
        }
    }
}

impl std::fmt::Display for AttachmentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttachmentState::NotDownloaded => write!(f, "·"),
            AttachmentState::Queued => write!(f, ">"),
            AttachmentState::Downloading => write!(f, "o"),
            AttachmentState::Downloaded => write!(f, "*"),
        }
    }
}
