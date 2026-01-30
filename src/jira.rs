use anyhow::Result;
use futures::stream::StreamExt;
use reqwest::{Client, IntoUrl};
use serde::Deserialize;
use tokio::{io::AsyncWriteExt, sync::watch::Sender};

/// A client for requesting Jira issue attachments.
///
/// Provides functionality to fetch attachments from Jira issues and download them.
#[derive(Debug, Clone)]
pub struct Jira {
    client: Client,
    base_url: String,
    auth: Auth,
}

/// Authentication method for Jira API requests.
#[derive(Debug, Clone)]
pub enum Auth {
    /// No authentication.
    None,
    /// Basic authentication with username and optional password.
    Basic { username: String, password: Option<String> },
    /// Bearer token authentication.
    Bearer { token: String },
}

#[derive(Debug, Deserialize)]
struct Issue {
    fields: Fields,
}

#[derive(Debug, Deserialize)]
struct Fields {
    attachment: Vec<Attachment>,
}

/// Represents a file attachment from a Jira issue.
#[derive(Debug, Deserialize, Clone)]
pub struct Attachment {
    /// The filename of the attachment.
    pub filename: String,
    /// The size of the attachment in bytes.
    pub size: u64,
    /// The creation date of the attachment.
    pub created: String,
    /// The content URL of the attachment.
    pub content: String,
}

/// Events emitted during the download of an attachment.
#[derive(Debug, Clone)]
pub enum DownloadEvent {
    /// Download is starting.
    Starting,
    /// Download is in progress.
    Progress { downloaded: u64, total: Option<u64> },
    /// Download has finished.
    Finished,
    /// An error occurred during download.
    Error { msg: String },
}

impl Jira {
    /// Creates a new Jira client.
    ///
    /// # Arguments
    ///
    /// * `base_url` - The base URL of the Jira instance (e.g., `https://jira.example.com`)
    /// * `auth` - The authentication method to use for API requests
    pub fn new(base_url: String, auth: Auth) -> Self {
        Self {
            client: Client::new(),
            base_url,
            auth,
        }
    }

    fn request(&self, url: impl IntoUrl) -> reqwest::RequestBuilder {
        let req = self.client.get(url);
        match &self.auth {
            Auth::Basic { username, password } => {
                req.basic_auth(username, password.clone())
            }
            Auth::Bearer { token } => {
                req.bearer_auth(token)
            }
            Auth::None => req,
        }
    }

    /// Fetches all attachments from a Jira issue.
    ///
    /// # Arguments
    ///
    /// * `issue` - The issue key (e.g., `PROJ-123`)
    ///
    /// # Returns
    ///
    /// A vector of attachments from the issue.
    ///
    /// # Errors
    ///
    /// Returns an error if the API request fails or the issue is not found.
    pub async fn fetch_attachments(&self, issue: &str) -> Result<Vec<Attachment>> {
        let url = format!(
            "{}/rest/api/2/issue/{}?fields=attachment",
            self.base_url.trim_end_matches('/'),
            issue
        );
        let req = self.request(&url);
        let res = req.send().await?;
        if !res.status().is_success() {
            return Err(anyhow::anyhow!("Failed to fetch issue: {}", res.status()));
        }
        let issue: Issue = res.json().await?;
        Ok(issue.fields.attachment)
    }

    /// Downloads an attachment and writes it to a file.
    ///
    /// Progress updates are sent through the provided channel.
    ///
    /// # Arguments
    ///
    /// * `url` - The URL of the attachment to download
    /// * `file` - The file to write the attachment content to
    /// * `tx` - A channel sender for download progress events
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the download completes successfully.
    ///
    /// # Errors
    ///
    /// Returns an error if the download fails or the file write operation fails.
    pub async fn download_attachment(
        &self,
        url: String,
        mut file: tokio::fs::File,
        tx: Sender<DownloadEvent>,
    ) -> Result<()> {
        let req = self.request(&url);
        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(anyhow::anyhow!("HTTP error: {}", resp.status()));
        }
        let total = resp.content_length();
        let mut stream = resp.bytes_stream();
        let mut downloaded: u64 = 0;

        loop {
            tokio::select! {
                _ = tx.closed() => {
                    // Download cancelled
                    break Err(anyhow::anyhow!("Download cancelled"))
                }
                chunk = stream.next() => {
                    if let Some(chunk) = chunk {
                        let chunk = chunk?;
                        downloaded += chunk.len() as u64;
                        file.write_all(&chunk).await?;
                        let _ = tx.send(DownloadEvent::Progress {
                            downloaded,
                            total,
                        });
                    } else {
                        let _ = tx.send(DownloadEvent::Finished);
                        break Ok(())
                    }
                }
            }
        }
    }
}
