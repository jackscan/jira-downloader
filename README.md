# Jira Downloader

A terminal-based TUI application for downloading attachments from Jira issues.

## Features

- **Interactive TUI** - Browse and manage attachments with a keyboard-driven interface
- **Batch Downloads** - Queue multiple attachments for sequential download
- **Secure Authentication** - Token-based authentication with Jira
- **Configurable** - Load settings from config files or environment variables

## Configuration

Configuration can be provided in two ways (in order of precedence):

### 1. Config File

Create a config file at one of these locations:
- Path specified via `--config` flag
- Default platform-specific config directory:
  - Linux/Unix: `~/.config/jira-downloader/config.toml`
  - macOS: `~/Library/Application Support/jira-downloader/config.toml`
  - Windows: `%APPDATA%\jira-downloader\config\config.toml`

**Config file format:**
```toml
base_url = "https://your-jira-instance.com"
user = "your-username"
token = "your-api-token"
```

### 2. Environment Variables

Set environment variables with the `JIRA_` prefix:
```bash
export JIRA_BASE_URL="https://your-jira-instance.com"
export JIRA_USER="your-username"
export JIRA_TOKEN="your-api-token"
```

## Usage

### Basic Usage

```bash
jira-downloader PROJ-123
```

### With Custom Config

```bash
jira-downloader --config /path/to/config PROJ-123
```

## Logging

Logs are written to a rolling daily log file in the project's data directory:
- Linux/Unix: `~/.local/share/jira-downloader/`
- macOS: `~/Library/Application Support/jira-downloader/`
- Windows: `%APPDATA%\jira-downloader\data`

Logs can be filtered by setting the log level via `--loglevel` flag.
