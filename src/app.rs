use crate::jira;
use crossterm::event::KeyEventKind;
use futures::{FutureExt, StreamExt};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Rect},
    style::{Modifier, Style},
    widgets::{Block, TableState},
};
use std::path::PathBuf;
use tokio::sync::watch;
use tracing::{debug, error, info};
use unicode_width::UnicodeWidthStr;

#[derive(Debug)]
pub struct App {
    issue: String,
    jira: crate::jira::Jira,
    table_state: TableState,
    folder: PathBuf,
    attachments: Vec<Attachment>,
    lengths: (usize, usize, usize, usize),
    exit: bool,
    download_ctrl: Option<DownloadCtrl>,
    status_message: Option<String>,
}

#[derive(Debug)]
struct DownloadCtrl {
    attachment_index: usize,
    progress_rx: watch::Receiver<jira::DownloadEvent>,
}

#[derive(Debug, Clone)]
struct Attachment {
    filename: String,
    size: usize,
    created: String,
    state: AttachmentState,
    content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentState {
    NotDownloaded,
    Queued,
    Downloading { downloaded: u64, total: Option<u64> },
    Downloaded,
    Failed { errmsg: String },
}

impl App {
    pub fn new(
        jira: crate::jira::Jira,
        issue: String,
        folder: PathBuf,
        attachments: Vec<crate::jira::Attachment>,
    ) -> Self {
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
            4, // State column width
            max_filename_width,
            max_size_width,
            max_created_width,
        );

        Self {
            issue,
            jira,
            table_state: TableState::default(),
            folder,
            attachments,
            lengths,
            exit: false,
            download_ctrl: None,
            status_message: None,
        }
    }

    pub async fn run(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        if let Err(err) = tokio::fs::create_dir_all(&self.folder).await {
            return Err(anyhow::anyhow!(
                "Failed to create download directory {:?}: {}",
                self.folder,
                err
            ));
        }

        // Init attachement state
        for att in self.attachments.iter_mut() {
            att.state = match tokio::fs::try_exists(self.folder.join(&att.filename)).await {
                Ok(true) => AttachmentState::Downloaded,
                Ok(false) => AttachmentState::NotDownloaded,
                Err(e) => AttachmentState::Failed {
                    errmsg: e.to_string(),
                },
            };
        }

        while !self.exit {
            let min_delay = tokio::time::sleep(std::time::Duration::from_millis(20));

            self.update_status_message();
            terminal.draw(|frame| {
                self.draw(frame);
            })?;

            let mut evt_reader = crossterm::event::EventStream::new();
            let progress_fut =
                self.download_ctrl
                    .as_mut()
                    .map_or(futures::future::pending().boxed(), |ctrl| {
                        async move {
                            if let Err(err) = ctrl.progress_rx.changed().await {
                                (
                                    ctrl.attachment_index,
                                    jira::DownloadEvent::Error {
                                        msg: err.to_string(),
                                    },
                                )
                            } else {
                                (ctrl.attachment_index, ctrl.progress_rx.borrow().clone())
                            }
                        }
                        .boxed()
                    });

            tokio::select! {
                (index, evt) = progress_fut => {
                    self.update_download(index, evt);
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

            min_delay.await;
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
            crossterm::event::KeyCode::Char(' ') => {
                self.toggle_selection();
            }
            crossterm::event::KeyCode::Enter => {
                self.start_downloads();
            }
            crossterm::event::KeyCode::Esc => {
                self.table_state.select(None);
            }
            crossterm::event::KeyCode::Tab => {
                self.table_state
                    .select(self.table_state.selected().map_or(Some(0), |_| None));
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

    fn toggle_selection(&mut self) {
        if let Some(selected) = self.table_state.selected() {
            let att = &mut self.attachments[selected];
            att.state = match att.state {
                AttachmentState::NotDownloaded | AttachmentState::Failed { errmsg: _ } => {
                    AttachmentState::Queued
                }
                AttachmentState::Queued => AttachmentState::NotDownloaded,
                ref state => state.clone(),
            };
        }
    }

    fn update_status_message(&mut self) {
        if let Some(i) = self.table_state.selected() {
            let att = &self.attachments[i];
            self.status_message = match &att.state {
                AttachmentState::NotDownloaded => {
                    Some(format!("Attachment '{}' is not downloaded.", att.filename))
                }
                AttachmentState::Queued => Some(format!(
                    "Attachment '{}' is queued for download.",
                    att.filename
                )),
                AttachmentState::Downloading { downloaded, total } => {
                    if let Some(total) = total {
                        Some(format!(
                            "Downloading '{}'... {}/{} bytes",
                            att.filename, downloaded, total
                        ))
                    } else {
                        Some(format!(
                            "Downloading '{}'... {} bytes downloaded",
                            att.filename, downloaded
                        ))
                    }
                }
                AttachmentState::Downloaded => Some(format!(
                    "Attachment '{}' has been downloaded.",
                    att.filename
                )),
                AttachmentState::Failed { errmsg } => Some(format!(
                    "Attachment '{}' failed to download: {}",
                    att.filename, errmsg
                )),
            };
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let toplayout =
            ratatui::layout::Layout::vertical([Constraint::Fill(1), Constraint::Max(1)])
                .split(frame.area());

        let layout = ratatui::layout::Layout::vertical([
            Constraint::Max(self.attachments.len() as u16 + 5),
            Constraint::Fill(1),
        ])
        .spacing(ratatui::layout::Spacing::Overlap(1))
        .split(toplayout[0]);

        self.render_table(frame, layout[0]);
        self.render_status(frame, layout[1]);
        self.render_help(frame, toplayout[1]);
    }

    fn render_table(&mut self, frame: &mut Frame, area: Rect) {
        let rows = self.attachments.iter().map(|att| {
            ratatui::widgets::Row::new(vec![
                ratatui::text::Line::from(att.state.to_string()).right_aligned(),
                att.filename.clone().into(),
                att.size.to_string().into(),
                att.created.clone().into(),
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
                .title(format!("{} Attachments", self.issue))
                .borders(ratatui::widgets::Borders::ALL)
                .merge_borders(ratatui::symbols::merge::MergeStrategy::Exact),
        )
        .row_highlight_style(selected_row_style);

        frame.render_stateful_widget(t, area, &mut self.table_state);
    }

    fn render_status(&self, frame: &mut Frame, area: Rect) {
        let paragraph =
            ratatui::widgets::Paragraph::new(self.status_message.clone().unwrap_or_default())
                .block(
                    Block::bordered().merge_borders(ratatui::symbols::merge::MergeStrategy::Exact),
                );
        frame.render_widget(paragraph, area);
    }

    fn render_help(&self, frame: &mut Frame, area: Rect) {
        let status_text =
            "q: Quit | ↑/↓: Navigate | Space: Select/Deselect | Enter: Start Download";
        let paragraph = ratatui::widgets::Paragraph::new(status_text)
            .style(Style::default().add_modifier(Modifier::REVERSED));
        frame.render_widget(paragraph, area);
    }

    fn start_downloads(&mut self) {
        if self.download_ctrl.is_some() {
            // download already in progress
            return;
        }

        if let Some((i, a)) = self
            .attachments
            .iter()
            .enumerate()
            .find(|(_, a)| a.state == AttachmentState::Queued)
        {
            let j = self.jira.clone();
            let url = a.content.clone();
            let file_path = self.folder.join(&a.filename);
            let (tx, rx) = watch::channel(jira::DownloadEvent::Starting);

            // spawn a tokio task to download
            tokio::spawn(async move {
                if let Err(e) = download_attachment(&j, url, file_path, tx.clone()).await {
                    let _ = tx.send(jira::DownloadEvent::Error { msg: e.to_string() });
                }
            });

            self.download_ctrl = Some(DownloadCtrl {
                attachment_index: i,
                progress_rx: rx,
            });
        };
    }

    fn update_download(&mut self, index: usize, evt: jira::DownloadEvent) {
        let att = &mut self.attachments[index];
        match evt {
            jira::DownloadEvent::Starting => {
                debug!("Starting download for {}", att.filename);
                att.state = AttachmentState::Downloading {
                    downloaded: 0,
                    total: None,
                };
            }
            jira::DownloadEvent::Progress { downloaded, total } => {
                att.state = AttachmentState::Downloading { downloaded, total };
            }
            jira::DownloadEvent::Finished => {
                info!("Download finished for {}", att.filename);
                att.state = AttachmentState::Downloaded;
                self.download_ctrl = None;
                self.start_downloads(); // start next download
            }
            jira::DownloadEvent::Error { msg } => {
                error!("Download error for {}: {}", att.filename, msg);
                att.state = AttachmentState::Failed { errmsg: msg };
                self.download_ctrl = None;
                self.start_downloads(); // start next download
            }
        }
    }
}

async fn create_tmp_download_file(
    file_path: &PathBuf,
) -> anyhow::Result<(tokio::fs::File, PathBuf)> {
    let mut tmp_file_path = file_path.clone();
    loop {
        tmp_file_path.add_extension("part");
        match tokio::fs::File::create_new(&tmp_file_path).await {
            Ok(file) => break Ok((file, tmp_file_path)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // try again with a new name
                continue;
            }
            Err(e) => {
                break Err(anyhow::anyhow!(
                    "Failed to create file {:?}: {}",
                    tmp_file_path,
                    e
                ));
            }
        }
    }
}

async fn download_attachment(
    jira: &crate::jira::Jira,
    url: String,
    file_path: PathBuf,
    tx: tokio::sync::watch::Sender<jira::DownloadEvent>,
) -> anyhow::Result<()> {
    let (tmp_file, tmp_file_path) = create_tmp_download_file(&file_path).await?;
    jira.download_attachment(url, tmp_file, tx).await?;
    tokio::fs::rename(tmp_file_path, file_path).await?;
    Ok(())
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
            AttachmentState::Downloading { downloaded, total } => {
                if let Some(total) = total {
                    let percent = *downloaded * 100 / *total;
                    write!(f, "{}%", percent)
                } else {
                    write!(f, "↓")
                }
            }
            AttachmentState::Downloaded => write!(f, "✓"),
            AttachmentState::Failed { errmsg: _ } => write!(f, "/!\\"),
        }
    }
}
