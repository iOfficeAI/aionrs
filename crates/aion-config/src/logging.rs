use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, serde::Serialize, Default)]
pub struct LoggingConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub dir: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedLogging {
    pub enabled: bool,
    pub level: String,
    pub dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum LoggingError {
    #[error("failed to create log directory '{path}': {source}")]
    CreateDir { path: PathBuf, source: std::io::Error },
    #[error("failed to build log file appender: {0}")]
    AppenderInit(String),
    #[error("invalid log level filter '{filter}': {reason}")]
    InvalidFilter { filter: String, reason: String },
}

pub fn default_log_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir()
            .map(|h| h.join("Library").join("Logs").join("aionrs"))
            .unwrap_or_else(|| PathBuf::from("aionrs/logs"))
    }
    #[cfg(target_os = "linux")]
    {
        dirs::state_dir()
            .map(|d| d.join("aionrs").join("logs"))
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .map(|h| h.join(".local").join("state").join("aionrs").join("logs"))
                    .unwrap_or_else(|| PathBuf::from("aionrs/logs"))
            })
    }
    #[cfg(target_os = "windows")]
    {
        dirs::data_dir()
            .map(|d| d.join("aionrs").join("logs"))
            .unwrap_or_else(|| PathBuf::from("aionrs/logs"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        PathBuf::from("aionrs/logs")
    }
}

pub use tracing_appender::non_blocking::WorkerGuard as LoggingGuard;

use tracing::Subscriber;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::fmt;
use tracing_subscriber::registry::LookupSpan;

pub fn create_file_layer<S>(
    config: &ResolvedLogging,
) -> Result<(Box<dyn Layer<S> + Send + Sync>, WorkerGuard), LoggingError>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    std::fs::create_dir_all(&config.dir).map_err(|source| LoggingError::CreateDir {
        path: config.dir.clone(),
        source,
    })?;

    let file_appender = tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_suffix("aionrs.log")
        .build(&config.dir)
        .map_err(|e| LoggingError::AppenderInit(e.to_string()))?;

    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_new(&config.level).map_err(|e| LoggingError::InvalidFilter {
        filter: config.level.clone(),
        reason: e.to_string(),
    })?;

    let layer = fmt::layer()
        .json()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .with_filter(filter);

    Ok((Box::new(layer), guard))
}

impl LoggingConfig {
    pub fn merge(global: Self, project: Self) -> Self {
        Self {
            enabled: project.enabled.or(global.enabled),
            level: project.level.or(global.level),
            dir: project.dir.or(global.dir),
        }
    }

    pub fn resolve(&self, cli_log_dir: Option<&str>, cli_log_level: Option<&str>) -> ResolvedLogging {
        let dir = cli_log_dir
            .map(PathBuf::from)
            .or_else(|| self.dir.as_ref().map(PathBuf::from))
            .unwrap_or_else(default_log_dir);

        let has_explicit_dir = cli_log_dir.is_some() || self.dir.is_some();
        let enabled = self.enabled.unwrap_or(has_explicit_dir);

        let level = cli_log_level
            .map(String::from)
            .or_else(|| self.level.clone())
            .unwrap_or_else(|| "info".to_string());

        ResolvedLogging { enabled, level, dir }
    }
}

#[cfg(test)]
#[path = "logging_test.rs"]
mod logging_test;
