use anyhow::Result;
use futures::stream::StreamExt;
use reqwest::{Client, IntoUrl};
use serde::Deserialize;
use tokio::{io::AsyncWriteExt, sync::watch::Sender};

#[derive(Debug, Clone)]
pub struct Jira {
    client: Client,
    base_url: String,
    auth: Auth,
}

#[derive(Debug, Clone)]
pub enum Auth {
    None,
    Basic { username: String, password: Option<String> },
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

#[derive(Debug, Deserialize, Clone)]
pub struct Attachment {
    pub filename: String,
    pub size: u64,
    pub created: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub enum DownloadEvent {
    Starting,
    Progress { downloaded: u64, total: Option<u64> },
    Finished,
    Error { msg: String },
}

impl Jira {
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
