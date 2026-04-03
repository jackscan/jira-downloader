#[path = "../src/jira.rs"]
mod jira;

use jira::{Auth, DownloadEvent, Jira};
use reqwest::Client;
use tempfile::NamedTempFile;
use tokio::sync::watch;
use wiremock::{
    matchers::{basic_auth, bearer_token, method, path},
    Mock, MockServer, ResponseTemplate,
};

const ISSUE_JSON: &str = r#"{
    "fields": {
        "attachment": [
            {
                "filename": "report.pdf",
                "size": 1024,
                "created": "2024-01-15T10:30:00.000+0000",
                "content": "ATTACHMENT_URL"
            },
            {
                "filename": "screenshot.png",
                "size": 2048,
                "created": "2024-01-16T14:20:00.000+0000",
                "content": "ATTACHMENT_URL_2"
            }
        ]
    }
}"#;

const EMPTY_ISSUE_JSON: &str = r#"{
    "fields": {
        "attachment": []
    }
}"#;

fn create_jira(base_url: String, auth: Auth) -> Jira {
    Jira::with_client(base_url, auth, Client::new())
}

#[tokio::test]
async fn fetch_attachments_returns_attachments_on_success() {
    let mock_server = MockServer::start().await;
    let attachment_url = format!("{}/secure/attachment/12345/report.pdf", mock_server.uri());
    let response_json = ISSUE_JSON.replace("ATTACHMENT_URL", &attachment_url);

    Mock::given(method("GET"))
        .and(path("/rest/api/2/issue/PROJ-123"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::from_str::<serde_json::Value>(&response_json).unwrap()),
        )
        .mount(&mock_server)
        .await;

    let jira = create_jira(mock_server.uri(), Auth::None);
    let attachments = jira.fetch_attachments("PROJ-123").await.unwrap();

    assert_eq!(attachments.len(), 2);
    assert_eq!(attachments[0].filename, "report.pdf");
    assert_eq!(attachments[0].size, 1024);
    assert_eq!(attachments[0].created, "2024-01-15T10:30:00.000+0000");
    assert_eq!(attachments[0].content, attachment_url);
    assert_eq!(attachments[1].filename, "screenshot.png");
    assert_eq!(attachments[1].size, 2048);
}

#[tokio::test]
async fn fetch_attachments_returns_error_on_404() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/rest/api/2/issue/NONEXISTENT"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock_server)
        .await;

    let jira = create_jira(mock_server.uri(), Auth::None);
    let result = jira.fetch_attachments("NONEXISTENT").await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("404"));
}

#[tokio::test]
async fn fetch_attachments_returns_empty_for_issue_without_attachments() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/rest/api/2/issue/PROJ-456"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::from_str::<serde_json::Value>(EMPTY_ISSUE_JSON).unwrap(),
        ))
        .mount(&mock_server)
        .await;

    let jira = create_jira(mock_server.uri(), Auth::None);
    let attachments = jira.fetch_attachments("PROJ-456").await.unwrap();

    assert!(attachments.is_empty());
}

#[tokio::test]
async fn basic_auth_sends_correct_headers() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/rest/api/2/issue/PROJ-789"))
        .and(basic_auth("testuser", "testpass"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::from_str::<serde_json::Value>(EMPTY_ISSUE_JSON).unwrap(),
        ))
        .mount(&mock_server)
        .await;

    let jira = create_jira(
        mock_server.uri(),
        Auth::Basic {
            username: "testuser".to_string(),
            password: Some("testpass".to_string()),
        },
    );
    let result = jira.fetch_attachments("PROJ-789").await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn bearer_token_sends_correct_headers() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/rest/api/2/issue/PROJ-789"))
        .and(bearer_token("my-secret-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::from_str::<serde_json::Value>(EMPTY_ISSUE_JSON).unwrap(),
        ))
        .mount(&mock_server)
        .await;

    let jira = create_jira(
        mock_server.uri(),
        Auth::Bearer {
            token: "my-secret-token".to_string(),
        },
    );
    let result = jira.fetch_attachments("PROJ-789").await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn anonymous_mode_sends_no_auth_headers() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/rest/api/2/issue/PROJ-789"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::from_str::<serde_json::Value>(EMPTY_ISSUE_JSON).unwrap(),
        ))
        .mount(&mock_server)
        .await;

    let jira = create_jira(mock_server.uri(), Auth::None);
    let result = jira.fetch_attachments("PROJ-789").await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn download_attachment_writes_file_content() {
    let mock_server = MockServer::start().await;
    let file_content = b"Hello, this is test content for the attachment.";
    let attachment_url = format!("{}/secure/attachment/12345/file.txt", mock_server.uri());

    Mock::given(method("GET"))
        .and(path("/secure/attachment/12345/file.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(file_content.to_vec())
                .append_header("content-length", file_content.len().to_string()),
        )
        .mount(&mock_server)
        .await;

    let jira = create_jira(mock_server.uri(), Auth::None);
    let temp_file = NamedTempFile::new().unwrap();
    let file_path = temp_file.path().to_path_buf();
    let file = tokio::fs::File::create(&file_path).await.unwrap();
    let (tx, _rx) = watch::channel(DownloadEvent::Starting);

    jira.download_attachment(attachment_url, file, tx)
        .await
        .unwrap();

    let content = std::fs::read(&file_path).unwrap();
    assert_eq!(content, file_content);
}

#[tokio::test]
async fn download_attachment_returns_error_on_404() {
    let mock_server = MockServer::start().await;
    let attachment_url = format!("{}/secure/attachment/99999/missing.txt", mock_server.uri());

    Mock::given(method("GET"))
        .and(path("/secure/attachment/99999/missing.txt"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock_server)
        .await;

    let jira = create_jira(mock_server.uri(), Auth::None);
    let temp_file = NamedTempFile::new().unwrap();
    let file_path = temp_file.path().to_path_buf();
    let file = tokio::fs::File::create(&file_path).await.unwrap();
    let (tx, _rx) = watch::channel(DownloadEvent::Starting);

    let result = jira
        .download_attachment(attachment_url, file, tx)
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn download_attachment_sends_progress_events() {
    let mock_server = MockServer::start().await;
    let file_content = b"Test content for progress tracking.";
    let attachment_url = format!("{}/secure/attachment/12345/data.bin", mock_server.uri());

    Mock::given(method("GET"))
        .and(path("/secure/attachment/12345/data.bin"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(file_content.to_vec())
                .append_header("content-length", file_content.len().to_string()),
        )
        .mount(&mock_server)
        .await;

    let jira = create_jira(mock_server.uri(), Auth::None);
    let temp_file = NamedTempFile::new().unwrap();
    let file_path = temp_file.path().to_path_buf();
    let file = tokio::fs::File::create(&file_path).await.unwrap();
    let (tx, rx) = watch::channel(DownloadEvent::Starting);

    jira.download_attachment(attachment_url, file, tx)
        .await
        .unwrap();

    let last_event = rx.borrow().clone();
    assert!(
        matches!(last_event, DownloadEvent::Finished),
        "Expected Finished event, got {:?}",
        last_event
    );
}

#[test]
fn download_event_error_variant_is_constructible() {
    let event = DownloadEvent::Error {
        msg: "test error".to_string(),
    };
    if let DownloadEvent::Error { msg } = event {
        assert_eq!(msg, "test error");
    } else {
        panic!("Expected Error variant");
    }
}

#[test]
fn download_event_progress_fields_are_accessible() {
    let event = DownloadEvent::Progress {
        downloaded: 1024,
        total: Some(2048),
    };
    if let DownloadEvent::Progress { downloaded, total } = event {
        assert_eq!(downloaded, 1024);
        assert_eq!(total, Some(2048));
    } else {
        panic!("Expected Progress variant");
    }
}

#[test]
fn jira_new_constructs_client() {
    let jira = Jira::new("http://localhost".to_string(), Auth::None);
    drop(jira);
}
