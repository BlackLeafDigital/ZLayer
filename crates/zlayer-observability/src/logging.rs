//! Structured logging with JSON/pretty output and file rotation

use std::io;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter,
};

use crate::config::{FileLoggingConfig, LogFormat, LoggingConfig, RotationStrategy};
use crate::error::Result;

/// Guard that must be held to keep the async file writer running
pub struct LogGuard {
    _guard: Option<WorkerGuard>,
}

impl LogGuard {
    fn new(guard: Option<WorkerGuard>) -> Self {
        Self { _guard: guard }
    }
}

/// Initialize logging with the given configuration
///
/// Returns a guard that must be held for the lifetime of the application
/// to ensure logs are flushed properly.
pub fn init_logging(config: &LoggingConfig) -> Result<LogGuard> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        if let Some(ref directives) = config.filter_directives {
            EnvFilter::new(directives)
        } else {
            EnvFilter::new(level_to_string(config.level))
        }
    });

    // Handle file logging setup
    let (file_writer, guard) = if let Some(file_config) = &config.file {
        let (writer, guard) = create_file_writer(file_config)?;
        (Some(writer), Some(guard))
    } else {
        (None, None)
    };

    // Initialize based on format and file configuration
    // We need separate branches because of tracing-subscriber's complex type system
    match (config.format, file_writer) {
        (LogFormat::Pretty, Some(file_writer)) => {
            let console_layer = fmt::layer()
                .with_writer(io::stdout)
                .with_target(config.include_target)
                .with_file(config.include_location)
                .with_line_number(config.include_location)
                .with_span_events(FmtSpan::CLOSE)
                .pretty();

            let file_layer = fmt::layer()
                .with_writer(file_writer)
                .with_target(config.include_target)
                .with_file(config.include_location)
                .with_line_number(config.include_location)
                .with_span_events(FmtSpan::CLOSE)
                .with_ansi(false)
                .json();

            tracing_subscriber::registry()
                .with(env_filter)
                .with(console_layer)
                .with(file_layer)
                .init();
        }
        (LogFormat::Pretty, None) => {
            let console_layer = fmt::layer()
                .with_writer(io::stdout)
                .with_target(config.include_target)
                .with_file(config.include_location)
                .with_line_number(config.include_location)
                .with_span_events(FmtSpan::CLOSE)
                .pretty();

            tracing_subscriber::registry()
                .with(env_filter)
                .with(console_layer)
                .init();
        }
        (LogFormat::Json, Some(file_writer)) => {
            let console_layer = fmt::layer()
                .with_writer(io::stdout)
                .with_target(config.include_target)
                .with_file(config.include_location)
                .with_line_number(config.include_location)
                .with_span_events(FmtSpan::CLOSE)
                .json();

            let file_layer = fmt::layer()
                .with_writer(file_writer)
                .with_target(config.include_target)
                .with_file(config.include_location)
                .with_line_number(config.include_location)
                .with_span_events(FmtSpan::CLOSE)
                .with_ansi(false)
                .json();

            tracing_subscriber::registry()
                .with(env_filter)
                .with(console_layer)
                .with(file_layer)
                .init();
        }
        (LogFormat::Json, None) => {
            let console_layer = fmt::layer()
                .with_writer(io::stdout)
                .with_target(config.include_target)
                .with_file(config.include_location)
                .with_line_number(config.include_location)
                .with_span_events(FmtSpan::CLOSE)
                .json();

            tracing_subscriber::registry()
                .with(env_filter)
                .with(console_layer)
                .init();
        }
        (LogFormat::Compact, Some(file_writer)) => {
            let console_layer = fmt::layer()
                .with_writer(io::stdout)
                .with_target(config.include_target)
                .with_file(config.include_location)
                .with_line_number(config.include_location)
                .with_span_events(FmtSpan::CLOSE)
                .compact();

            let file_layer = fmt::layer()
                .with_writer(file_writer)
                .with_target(config.include_target)
                .with_file(config.include_location)
                .with_line_number(config.include_location)
                .with_span_events(FmtSpan::CLOSE)
                .with_ansi(false)
                .json();

            tracing_subscriber::registry()
                .with(env_filter)
                .with(console_layer)
                .with(file_layer)
                .init();
        }
        (LogFormat::Compact, None) => {
            let console_layer = fmt::layer()
                .with_writer(io::stdout)
                .with_target(config.include_target)
                .with_file(config.include_location)
                .with_line_number(config.include_location)
                .with_span_events(FmtSpan::CLOSE)
                .compact();

            tracing_subscriber::registry()
                .with(env_filter)
                .with(console_layer)
                .init();
        }
    }

    Ok(LogGuard::new(guard))
}

fn level_to_string(level: crate::config::LogLevel) -> String {
    match level {
        crate::config::LogLevel::Trace => "trace",
        crate::config::LogLevel::Debug => "debug",
        crate::config::LogLevel::Info => "info",
        crate::config::LogLevel::Warn => "warn",
        crate::config::LogLevel::Error => "error",
    }
    .to_string()
}

fn create_file_writer(
    config: &FileLoggingConfig,
) -> Result<(tracing_appender::non_blocking::NonBlocking, WorkerGuard)> {
    let file_appender = match config.rotation {
        RotationStrategy::Daily => {
            tracing_appender::rolling::daily(&config.directory, &config.prefix)
        }
        RotationStrategy::Hourly => {
            tracing_appender::rolling::hourly(&config.directory, &config.prefix)
        }
        RotationStrategy::Never => {
            tracing_appender::rolling::never(&config.directory, &config.prefix)
        }
    };

    Ok(tracing_appender::non_blocking(file_appender))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_conversion() {
        assert_eq!(level_to_string(crate::config::LogLevel::Info), "info");
        assert_eq!(level_to_string(crate::config::LogLevel::Debug), "debug");
        assert_eq!(level_to_string(crate::config::LogLevel::Trace), "trace");
        assert_eq!(level_to_string(crate::config::LogLevel::Warn), "warn");
        assert_eq!(level_to_string(crate::config::LogLevel::Error), "error");
    }

    #[test]
    fn test_log_guard_creation() {
        let guard = LogGuard::new(None);
        assert!(guard._guard.is_none());
    }
}
