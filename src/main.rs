use std::{env::current_dir, path::PathBuf};

use anyhow::{Result, anyhow};
use clap::Parser;
use config::{Config, File};
use directories::ProjectDirs;
use tracing::{debug, info};

mod app;
mod jira;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to config file
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Issue key to download
    #[arg(value_name = "ISSUE")]
    issue: String,
    /// Log level (error, warn, info, debug, trace)
    #[arg(short, long, default_value = "info")]
    loglevel: tracing::Level,
}

#[derive(Debug, serde::Deserialize)]
struct Settings {
    base_url: String,
    user: Option<String>,
    token: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let logfolder = project_directory()
        .map(|pdir| pdir.data_dir().to_path_buf())
        .unwrap_or_else(|| current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let logfile =
        tracing_appender::rolling::daily(logfolder, format!("{}.log", env!("CARGO_PKG_NAME")));

    tracing_subscriber::fmt()
        .with_max_level(args.loglevel)
        .with_writer(logfile)
        .init();

    let config_builder = Config::builder();
    let config_builder = if let Some(config_path) = args.config {
        if !config_path.exists() {
            return Err(anyhow!("Config file not found at {:?}", config_path));
        }
        debug!("Loading config from {config_path:?}");
        // Load config from specified location
        config_builder.add_source(File::from(config_path))
    } else if let Some(config_path) = args
        .config
        .or(project_directory().map(|pdir| pdir.config_dir().join("config")))
    {
        // Load config from default location if it exists
        debug!("Looking for config at {config_path:?}");
        config_builder.add_source(File::from(config_path).required(false))
    } else {
        config_builder
    };

    let config = config_builder
        .add_source(config::Environment::with_prefix("JIRA"))
        .build()?;
    let settings = config.try_deserialize::<Settings>()?;

    let (authdesc, authmethod) = match (&settings.user, &settings.token) {
        (Some(user), token) => (format!("Basic: {}", user), jira::Auth::Basic {
            username: user.clone(),
            password: token.clone(),
        }),
        (None, Some(token)) => ("Bearer token".to_string(), jira::Auth::Bearer {
            token: token.clone(),
        }),
        (None, None) => ("Anonymous access".to_string(), jira::Auth::None),
    };

    info!("Jira Base: {}, Auth: {}", settings.base_url, authdesc);

    let jira = jira::Jira::new(settings.base_url, authmethod);

    let attachments = jira.fetch_attachments(&args.issue).await?;
    for att in &attachments {
        info!(
            "Attachment: \"{}\" ({} bytes) - {}",
            att.filename, att.size, att.created
        );
    }

    let mut app = app::App::new(jira, args.issue.clone(), args.issue.into(), attachments);
    let mut terminal = ratatui::init();
    app.run(&mut terminal).await?;
    ratatui::restore();

    Ok(())
}

fn project_directory() -> Option<ProjectDirs> {
    ProjectDirs::from("", "", env!("CARGO_PKG_NAME"))
}
