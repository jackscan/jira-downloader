use axum::{
    body::Bytes,
    extract::Path,
    http::{HeaderValue, StatusCode},
    response::Response,
    routing::get,
    Router,
};
use futures::stream;
use std::time::Duration;

const PORT: u16 = 8080;
const CHUNK_SIZE: usize = 64 * 1024; // 64 KB per chunk
const CHUNK_DELAY: Duration = Duration::from_millis(100); // 100 ms between chunks

struct AttachmentInfo {
    id: usize,
    filename: String,
    size: usize,
    created: String,
}

fn sample_attachments() -> Vec<AttachmentInfo> {
    vec![
        AttachmentInfo {
            id: 10001,
            filename: "report.pdf".to_string(),
            size: 2 * 1024 * 1024, // 2 MB
            created: "2024-01-15T10:30:00.000+0000".to_string(),
        },
        AttachmentInfo {
            id: 10002,
            filename: "screenshot.png".to_string(),
            size: 5 * 1024 * 1024, // 5 MB
            created: "2024-01-16T14:20:00.000+0000".to_string(),
        },
        AttachmentInfo {
            id: 10003,
            filename: "notes.txt".to_string(),
            size: 512 * 1024, // 512 KB
            created: "2024-01-17T09:00:00.000+0000".to_string(),
        },
    ]
}

fn generate_content(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 256) as u8).collect()
}

fn build_issue_json() -> serde_json::Value {
    let attachments: Vec<serde_json::Value> = sample_attachments()
        .iter()
        .map(|a| {
            serde_json::json!({
                "filename": a.filename,
                "size": a.size,
                "created": a.created,
                "content": format!("http://127.0.0.1:{}/secure/attachment/{}/{}", PORT, a.id, a.filename)
            })
        })
        .collect();

    serde_json::json!({
        "fields": {
            "attachment": attachments
        }
    })
}

async fn get_issue() -> (StatusCode, axum::Json<serde_json::Value>) {
    (StatusCode::OK, axum::Json(build_issue_json()))
}

async fn get_attachment(Path((id, filename)): Path<(usize, String)>) -> Response {
    let attachments = sample_attachments();

    let Some(att) = attachments.iter().find(|a| a.id == id && a.filename == filename) else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(axum::body::Body::from("Attachment not found"))
            .unwrap();
    };

    let content = generate_content(att.size);
    let total_size = content.len();

    // Split content into fixed-size chunks and yield each with a delay,
    // simulating a slow network connection so download progress is observable.
    let chunks: Vec<Bytes> = content
        .chunks(CHUNK_SIZE)
        .map(|c| Bytes::copy_from_slice(c))
        .collect();

    let throttled = stream::unfold(chunks.into_iter(), |mut iter: std::vec::IntoIter<Bytes>| async move {
        let chunk = iter.next()?;
        tokio::time::sleep(CHUNK_DELAY).await;
        Some((Ok::<Bytes, std::convert::Infallible>(chunk), iter))
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(
            "Content-Disposition",
            HeaderValue::from_str(&format!("attachment; filename=\"{}\"", att.filename)).unwrap(),
        )
        .header("Content-Length", total_size.to_string())
        .body(axum::body::Body::from_stream(throttled))
        .unwrap()
}

async fn health() -> &'static str {
    "ok"
}

#[tokio::main]
async fn main() {
    let attachments = sample_attachments();
    println!("Starting mock Jira server on http://127.0.0.1:{}", PORT);
    println!("Available issue: PROJ-123");
    println!("Attachments:");
    for att in &attachments {
        println!("  - {} ({} bytes, {})", att.filename, att.size, att.created);
    }
    println!();
    println!("Run the downloader with:");
    println!(
        "  JIRA_BASE_URL=http://127.0.0.1:{} cargo run -- PROJ-123",
        PORT
    );

    let app = Router::new()
        .route("/health", get(health))
        .route("/rest/api/2/issue/{key}", get(get_issue))
        .route("/secure/attachment/{id}/{filename}", get(get_attachment));

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", PORT))
        .await
        .unwrap_or_else(|e| panic!("Failed to bind to port {}: {}", PORT, e));

    axum::serve(listener, app).await.unwrap();
}
