use anyhow::Result;
use reqwest::Client;
use serde::Deserialize;

pub struct Jira {
    client: Client,
    base_url: String,
    auth: Option<(String, String)>,
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

impl Jira {
    pub fn new(base_url: String, auth: Option<(String, String)>) -> Self {
        Self {
            client: Client::new(),
            base_url,
            auth,
        }
    }

    pub async fn fetch_attachments(
        &self,
        issue: &str,
    ) -> Result<Vec<Attachment>> {
        let url = format!(
            "{}/rest/api/2/issue/{}?fields=attachment",
            self.base_url.trim_end_matches('/'),
            issue
        );
        let mut req = self.client.get(&url);
        if let Some((user, token)) = &self.auth {
            req = req.basic_auth(user, Some(token));
        }
        let res = req.send().await?;
        if !res.status().is_success() {
            return Err(anyhow::anyhow!("Failed to fetch issue: {}", res.status()));
        }
        let issue: Issue = res.json().await?;
        Ok(issue.fields.attachment)
    }
}
