use crate::{filter, jira};
use crossterm::event::{KeyCode, KeyEventKind};
use futures::{FutureExt, StreamExt, TryFutureExt};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, TableState},
};
use std::path::PathBuf;
use tokio::sync::watch;
use tracing::{debug, error, info};
use unicode_width::UnicodeWidthStr;

/// The main application state and logic.
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
    download_batch: Option<DownloadBatch>,
    status_message: Option<String>,
    status_feedback: Option<String>,
    filter_input: filter::FilterInputState,
}

#[derive(Debug)]
struct DownloadCtrl {
    attachment_index: usize,
    progress_rx: watch::Receiver<jira::DownloadEvent>,
}

#[derive(Debug)]
struct DownloadBatch {
    completed_files: usize,
    completed_bytes: u64,
    start_time: std::time::Instant,
}

#[derive(Debug, Clone)]
struct Attachment {
    filename: String,
    size: usize,
    created: String,
    state: AttachmentState,
    content: String,
}

/// The state of an attachment in the download process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentState {
    /// The attachment has not been downloaded yet.
    NotDownloaded,
    /// The attachment is queued for download.
    Queued,
    /// The attachment is currently being downloaded.
    Downloading { downloaded: u64, total: Option<u64> },
    /// The attachment has been downloaded.
    Downloaded,
    /// The attachment failed to download.
    Failed { errmsg: String },
}

impl App {
    /// Creates a new App instance.
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
            .map(|att| format_file_size(att.size as u64).width())
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
            download_batch: None,
            status_message: None,
            status_feedback: None,
            filter_input: filter::FilterInputState::default(),
        }
    }

    /// Runs the main application loop.
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

        // Main loop
        let mut evt_reader = crossterm::event::EventStream::new();
        while !self.exit {
            let min_delay = tokio::time::sleep(std::time::Duration::from_millis(20));

            self.update_selected_status_message();
            terminal.draw(|frame| {
                self.draw(frame);
            })?;

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
                                (
                                    ctrl.attachment_index,
                                    ctrl.progress_rx.borrow_and_update().clone(),
                                )
                            }
                        }
                        .boxed()
                    });

            tokio::select! {
                biased;
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
                (index, evt) = progress_fut => {
                    self.update_download(index, evt);
                }
            }

            min_delay.await;
        }

        Ok(())
    }

    fn handle_key_press(&mut self, key_evt: crossterm::event::KeyEvent) {
        if self.filter_input.is_active {
            self.handle_filter_key_press(key_evt);
        } else {
            self.handle_table_key_press(key_evt);
        }
    }

    fn handle_table_key_press(&mut self, key_evt: crossterm::event::KeyEvent) {
        match key_evt.code {
            KeyCode::Char('q') => {
                self.exit = true;
            }
            KeyCode::Up => {
                self.previous_row();
            }
            KeyCode::Down => {
                self.next_row();
            }
            KeyCode::Char(' ') => {
                self.toggle_selection();
            }
            KeyCode::Enter => {
                self.start_downloads();
            }
            KeyCode::Esc => {
                self.table_state.select(None);
            }
            KeyCode::Tab => {
                self.table_state
                    .select(self.table_state.selected().map_or(Some(0), |_| None));
            }
            KeyCode::Char('/') => {
                self.activate_filter_input();
            }
            _ => {}
        }
    }

    fn handle_filter_key_press(&mut self, key_evt: crossterm::event::KeyEvent) {
        match key_evt.code {
            KeyCode::Char(ch) => {
                self.status_feedback = None;
                self.filter_input
                    .draft
                    .insert(self.filter_input.cursor_index, ch);
                self.filter_input.cursor_index += ch.len_utf8();
                self.recompute_filter_preview();
            }
            KeyCode::Backspace => {
                self.status_feedback = None;
                if self.filter_input.cursor_index > 0 {
                    let start = previous_char_boundary(
                        &self.filter_input.draft,
                        self.filter_input.cursor_index,
                    );
                    self.filter_input
                        .draft
                        .drain(start..self.filter_input.cursor_index);
                    self.filter_input.cursor_index = start;
                    self.recompute_filter_preview();
                }
            }
            KeyCode::Enter => {
                self.status_feedback = None;
                self.apply_filter_confirm();
            }
            KeyCode::Esc => {
                self.status_feedback = None;
                self.filter_input.cancel();
            }
            _ => {}
        }
    }

    fn next_row(&mut self) {
        if self.attachments.is_empty() {
            self.table_state.select(None);
            return;
        }
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

    fn update_selected_status_message(&mut self) {
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
                            "Downloading '{}'... {}/{}",
                            att.filename,
                            format_file_size(*downloaded),
                            format_file_size(*total)
                        ))
                    } else {
                        Some(format!(
                            "Downloading '{}'... {} downloaded",
                            att.filename,
                            format_file_size(*downloaded)
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
        } else {
            self.status_message = None;
        }
    }

    fn activate_filter_input(&mut self) {
        self.filter_input.activate();
        self.recompute_filter_preview();
    }

    fn recompute_filter_preview(&mut self) {
        let pattern = filter::FilterPattern::from_input(&self.filter_input.draft);
        match filter::matching_indices(
            &pattern,
            self.attachments.iter().map(|att| att.filename.as_str()),
        ) {
            Ok(indices) => {
                self.filter_input.preview_matches = indices;
                self.filter_input.compile_error = None;
            }
            Err(errmsg) => {
                self.filter_input.preview_matches.clear();
                self.filter_input.compile_error = Some(errmsg);
            }
        }
    }

    fn apply_filter_confirm(&mut self) {
        let pattern = filter::FilterPattern::from_input(&self.filter_input.draft);
        let pattern_text = pattern.raw.clone();

        if matches!(pattern.kind, filter::PatternKind::Empty) {
            self.status_feedback = Some(filter::empty_input_result().message);
            self.filter_input.cancel();
            return;
        }

        let matched_indices = match filter::matching_indices(
            &pattern,
            self.attachments.iter().map(|att| att.filename.as_str()),
        ) {
            Ok(indices) => indices,
            Err(errmsg) => {
                self.status_feedback =
                    Some(filter::invalid_glob_result(&pattern_text, &errmsg).message);
                self.filter_input.cancel();
                return;
            }
        };

        let mut eligible_total = 0usize;
        let mut queued_total = 0usize;
        let mut skipped_total = 0usize;

        for index in matched_indices.iter().copied() {
            if let Some(att) = self.attachments.get_mut(index) {
                if filter::is_queue_eligible(queue_state_from_attachment(&att.state)) {
                    eligible_total += 1;
                    att.state = AttachmentState::Queued;
                    queued_total += 1;
                } else {
                    skipped_total += 1;
                }
            }
        }

        let apply_result = filter::build_apply_result(
            &pattern_text,
            matched_indices.len(),
            eligible_total,
            queued_total,
            skipped_total,
        );

        self.status_feedback = Some(apply_result.message);
        self.filter_input.cancel();
    }

    fn draw(&mut self, frame: &mut Frame) {
        let toplayout =
            ratatui::layout::Layout::vertical([Constraint::Fill(1), Constraint::Max(1)])
                .split(frame.area());

        let show_batch = self.download_batch.is_some();

        let constraints: Vec<Constraint> = if show_batch {
            vec![
                Constraint::Max(self.attachments.len() as u16 + 5),
                Constraint::Max(3),
                Constraint::Fill(1),
            ]
        } else {
            vec![
                Constraint::Max(self.attachments.len() as u16 + 5),
                Constraint::Fill(1),
            ]
        };

        let layout = ratatui::layout::Layout::vertical(constraints)
            .spacing(ratatui::layout::Spacing::Overlap(1))
            .split(toplayout[0]);

        self.render_table(frame, layout[0]);
        if show_batch {
            self.render_download_progress(frame, layout[1]);
            self.render_status(frame, layout[2]);
        } else {
            self.render_status(frame, layout[1]);
        }
        self.render_help(frame, toplayout[1]);
    }

    fn render_table(&mut self, frame: &mut Frame, area: Rect) {
        let rows = self.attachments.iter().enumerate().map(|(index, att)| {
            let mut row = ratatui::widgets::Row::new(vec![
                ratatui::text::Line::from(att.state.to_string()).right_aligned(),
                att.filename.clone().into(),
                format_file_size(att.size as u64).into(),
                att.created.clone().into(),
            ]);

            if self.filter_input.is_active && self.filter_input.preview_matches.contains(&index) {
                row = row.style(Style::default().fg(Color::Cyan));
            }

            row
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
        let status_text = if self.filter_input.is_active {
            if let Some(errmsg) = &self.filter_input.compile_error {
                format!(
                    "Filter: {} | invalid glob: {}",
                    self.filter_input.draft, errmsg
                )
            } else {
                format!(
                    "Filter: {} | {} match(es)",
                    self.filter_input.draft,
                    self.filter_input.preview_matches.len()
                )
            }
        } else if let Some(feedback) = &self.status_feedback {
            feedback.clone()
        } else {
            self.status_message.clone().unwrap_or_default()
        };

        let paragraph = ratatui::widgets::Paragraph::new(status_text)
            .block(Block::bordered().merge_borders(ratatui::symbols::merge::MergeStrategy::Exact));
        frame.render_widget(paragraph, area);
    }

    fn render_help(&self, frame: &mut Frame, area: Rect) {
        let status_text = if self.filter_input.is_active {
            "Filter mode | Type: Pattern | Backspace: Delete | Enter: Apply | Esc: Cancel"
        } else {
            "q: Quit | ↑/↓: Navigate | Space: Select | /: Filter | Enter: Start Download"
        };
        let paragraph = ratatui::widgets::Paragraph::new(status_text)
            .style(Style::default().add_modifier(Modifier::REVERSED));
        frame.render_widget(paragraph, area);
    }

    fn render_download_progress(&self, frame: &mut Frame, area: Rect) {
        let Some(batch) = &self.download_batch else {
            return;
        };

        // Count queued files and sum their sizes.
        let queued_files = self
            .attachments
            .iter()
            .filter(|a| a.state == AttachmentState::Queued)
            .count();
        let queued_bytes: u64 = self
            .attachments
            .iter()
            .filter(|a| a.state == AttachmentState::Queued)
            .map(|a| a.size as u64)
            .sum();

        // Current file progress.
        let (current_downloaded, current_total) = self
            .download_ctrl
            .as_ref()
            .and_then(|ctrl| {
                let att = &self.attachments[ctrl.attachment_index];
                if let AttachmentState::Downloading { downloaded, total } = &att.state {
                    Some((*downloaded, *total))
                } else {
                    None
                }
            })
            .unwrap_or((0, None));

        let is_downloading = self.download_ctrl.is_some();
        let total_files =
            batch.completed_files + queued_files + if is_downloading { 1 } else { 0 };
        let current_file_num = batch.completed_files + if is_downloading { 1 } else { 0 };

        let total_bytes = batch.completed_bytes
            + queued_bytes
            + current_total.unwrap_or_else(|| {
                self.download_ctrl
                    .as_ref()
                    .map(|ctrl| self.attachments[ctrl.attachment_index].size as u64)
                    .unwrap_or(0)
            });
        let transferred = batch.completed_bytes + current_downloaded;

        let elapsed = batch.start_time.elapsed().as_secs_f64();
        let speed = if elapsed >= 0.5 && transferred > 0 {
            Some(transferred as f64 / elapsed)
        } else {
            None
        };

        let text = if total_bytes > 0 {
            let base = format!(
                "File {}/{} | {} / {}",
                current_file_num,
                total_files,
                format_file_size(transferred),
                format_file_size(total_bytes),
            );
            match speed {
                Some(s) if s > 0.0 => {
                    let remaining = total_bytes.saturating_sub(transferred);
                    let eta = remaining as f64 / s;
                    format!(
                        "{} @ {} (~{} remaining)",
                        base,
                        format_speed(s),
                        format_duration(eta)
                    )
                }
                _ => base,
            }
        } else {
            let base = format!(
                "File {}/{} | {} downloaded",
                current_file_num,
                total_files,
                format_file_size(transferred),
            );
            match speed {
                Some(s) => format!(
                    "{} @ {} ({} elapsed)",
                    base,
                    format_speed(s),
                    format_duration(elapsed)
                ),
                None => base,
            }
        };

        let paragraph = ratatui::widgets::Paragraph::new(text)
            .block(Block::bordered().merge_borders(ratatui::symbols::merge::MergeStrategy::Exact));
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
            // Create a new batch if one isn't already running.
            if self.download_batch.is_none() {
                self.download_batch = Some(DownloadBatch {
                    completed_files: 0,
                    completed_bytes: 0,
                    start_time: std::time::Instant::now(),
                });
            }

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
        } else {
            // No more queued files — batch is done.
            self.download_batch = None;
        }
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
                let size = att.size as u64;
                att.state = AttachmentState::Downloaded;
                if let Some(batch) = &mut self.download_batch {
                    batch.completed_files += 1;
                    batch.completed_bytes += size;
                }
                self.download_ctrl = None;
                self.start_downloads(); // start next download
            }
            jira::DownloadEvent::Error { msg } => {
                error!("Download error for {}: {}", att.filename, msg);
                let size = att.size as u64;
                att.state = AttachmentState::Failed { errmsg: msg };
                if let Some(batch) = &mut self.download_batch {
                    batch.completed_files += 1;
                    batch.completed_bytes += size;
                }
                self.download_ctrl = None;
                self.start_downloads(); // start next download
            }
        }
    }
}

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .last()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn queue_state_from_attachment(state: &AttachmentState) -> filter::AttachmentQueueState {
    match state {
        AttachmentState::NotDownloaded => filter::AttachmentQueueState::NotDownloaded,
        AttachmentState::Failed { errmsg: _ } => filter::AttachmentQueueState::Failed,
        AttachmentState::Queued
        | AttachmentState::Downloading {
            downloaded: _,
            total: _,
        }
        | AttachmentState::Downloaded => filter::AttachmentQueueState::Other,
    }
}

async fn create_tmp_download_file(
    file_path: &PathBuf,
) -> anyhow::Result<(tokio::fs::File, PathBuf)> {
    let mut tmp_file_path = file_path.clone();
    let mut counter = 0;
    loop {
        if counter == 0 {
            let mut name = file_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("download")
                .to_string();
            name.push_str(".part");
            tmp_file_path.set_file_name(&name);
        } else {
            let mut name = file_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("download")
                .to_string();
            name.push_str(&format!(".part.{}", counter));
            tmp_file_path.set_file_name(&name);
        }
        match tokio::fs::File::create_new(&tmp_file_path).await {
            Ok(file) => break Ok((file, tmp_file_path)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                counter += 1;
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
    let tmp_path_to_remove = tmp_file_path.clone();

    if let Err(e) = jira
        .download_attachment(url, tmp_file, tx)
        .and_then(|()| async move {
            tokio::fs::rename(&tmp_file_path, file_path)
                .await
                .map_err(Into::into)
        })
        .await
    {
        let _ = tokio::fs::remove_file(&tmp_path_to_remove).await;
        return Err(e);
    }

    Ok(())
}

impl From<crate::jira::Attachment> for Attachment {
    fn from(att: crate::jira::Attachment) -> Self {
        Self {
            filename: att.filename,
            size: att.size as usize,
            created: chrono::DateTime::parse_from_str(&att.created, "%Y-%m-%dT%H:%M:%S%.3f%z")
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

pub fn format_file_size(size: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB", "PB"];
    let mut size = size as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    if unit_idx == 0 {
        format!("{} {}", size as u64, UNITS[unit_idx])
    } else {
        format!("{:.2} {}", size, UNITS[unit_idx])
    }
}

fn format_speed(bytes_per_sec: f64) -> String {
    const UNITS: &[&str] = &["B/s", "KB/s", "MB/s", "GB/s", "TB/s"];
    let mut val = bytes_per_sec;
    let mut unit_idx = 0;

    while val >= 1024.0 && unit_idx < UNITS.len() - 1 {
        val /= 1024.0;
        unit_idx += 1;
    }

    if unit_idx == 0 {
        format!("{} {}", val as u64, UNITS[unit_idx])
    } else {
        format!("{:.2} {}", val, UNITS[unit_idx])
    }
}

fn format_duration(secs: f64) -> String {
    let secs = secs as u64;
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    }
}
