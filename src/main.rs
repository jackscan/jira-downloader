use std::path::PathBuf;

use anyhow::{Result, anyhow};
use clap::Parser;
use config::{Config, File};
use directories::ProjectDirs;
use tracing::{debug, info};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to config file
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Issue key to download
    #[arg(value_name = "ISSUE")]
    issue: String,
    /// Log level (overrides config)
    #[arg(short, long, default_value = "info")]
    loglevel: tracing::Level,
}

#[derive(Debug, serde::Deserialize)]
struct Settings {
    base_url: String,
    user: String,
    #[allow(dead_code)]
    token: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_max_level(args.loglevel)
        .init();

    let config_builder = Config::builder();
    let config_builder = if let Some(config_path) = args.config {
        if !config_path.exists() {
            return Err(anyhow!("Config file not found at {:?}", config_path));
        }
        debug!("Loading config from {config_path:?}");
        // Load config from specified location
        config_builder.add_source(File::from(config_path))
    } else if let Some(config_path) =
        args.config.or(ProjectDirs::from("", "", "jira-downloader")
            .map(|pdir| pdir.config_dir().join("config")))
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

    info!("Jira Base: {}, User: {}", settings.base_url, settings.user);

    Ok(())
}
