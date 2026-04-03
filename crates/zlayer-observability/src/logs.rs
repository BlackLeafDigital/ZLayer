//! Structured container and execution log types.
//!
//! Provides a unified [`LogEntry`] type for all log sources (containers,
//! jobs, builds, daemon) with timestamps, stream identification, and
//! source metadata.

use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single structured log entry from any source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// When the log line was captured.
    pub timestamp: DateTime<Utc>,
    /// Which output stream produced this line.
    pub stream: LogStream,
    /// The log message content.
    pub message: String,
    /// Where this log entry originated.
    pub source: LogSource,
    /// Service name (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// Deployment name (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployment: Option<String>,
}

/// Output stream identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    Stdout,
    Stderr,
}

/// Identifies where a log entry originated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "lowercase")]
pub enum LogSource {
    /// Output from a running container.
    Container(String),
    /// Output from a job/function execution.
    Job(String),
    /// Output from an image build.
    Build(String),
    /// `ZLayer` daemon's own logs.
    Daemon,
}

/// Query filter for reading logs.
#[derive(Debug, Clone, Default)]
pub struct LogQuery {
    /// Filter by source type/ID.
    pub source: Option<LogSource>,
    /// Filter by stream (stdout only, stderr only, or both if None).
    pub stream: Option<LogStream>,
    /// Only entries after this timestamp.
    pub since: Option<DateTime<Utc>>,
    /// Only entries before this timestamp.
    pub until: Option<DateTime<Utc>>,
    /// Return at most this many entries (from the end).
    pub tail: Option<usize>,
}

impl std::fmt::Display for LogEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.stream, self.message)
    }
}

impl std::fmt::Display for LogStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stdout => write!(f, "stdout"),
            Self::Stderr => write!(f, "stderr"),
        }
    }
}

// ---------------------------------------------------------------------------
// Writers
// ---------------------------------------------------------------------------

/// Writes structured log entries to a JSONL file on disk.
///
/// Each line in the output file is a single JSON-serialized [`LogEntry`].
/// The file is opened in append mode so existing content is preserved across
/// restarts.
pub struct FileLogWriter {
    path: PathBuf,
    writer: Mutex<BufWriter<std::fs::File>>,
}

impl FileLogWriter {
    /// Opens (or creates) the file at `path` in append mode.
    ///
    /// # Errors
    ///
    /// Returns an IO error if the file cannot be opened or created.
    pub fn new(path: impl Into<PathBuf>) -> std::io::Result<Self> {
        let path = path.into();
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            path,
            writer: Mutex::new(BufWriter::new(file)),
        })
    }

    /// Serializes `entry` as JSON and writes it as a single line.
    ///
    /// The underlying buffer is flushed after every entry so that
    /// readers see output promptly.
    ///
    /// # Errors
    ///
    /// Returns an IO error if serialization or writing fails.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    pub fn write_entry(&self, entry: &LogEntry) -> std::io::Result<()> {
        let line = serde_json::to_string(entry)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut w = self.writer.lock().expect("FileLogWriter lock poisoned");
        w.write_all(line.as_bytes())?;
        w.write_all(b"\n")?;
        w.flush()
    }

    /// Convenience method that constructs a [`LogEntry`] with
    /// `Utc::now()` as the timestamp, then writes it.
    ///
    /// # Errors
    ///
    /// Returns an IO error if writing fails.
    pub fn write_line(
        &self,
        stream: LogStream,
        message: &str,
        source: LogSource,
    ) -> std::io::Result<()> {
        let entry = LogEntry {
            timestamp: Utc::now(),
            stream,
            message: message.to_string(),
            source,
            service: None,
            deployment: None,
        };
        self.write_entry(&entry)
    }

    /// Returns the file path this writer is appending to.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// In-memory ring buffer for log entries with a configurable capacity.
///
/// Once the buffer reaches `max_entries` the oldest entry is evicted for
/// every new entry written. This is useful for WASM containers and
/// short-lived executions where disk I/O is unavailable or undesirable.
pub struct MemoryLogWriter {
    entries: Mutex<VecDeque<LogEntry>>,
    max_entries: usize,
}

impl MemoryLogWriter {
    /// Creates a new ring buffer that holds at most `max_entries` entries.
    #[must_use]
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Mutex::new(VecDeque::with_capacity(max_entries)),
            max_entries,
        }
    }

    /// Pushes an entry into the buffer, evicting the oldest entry if the
    /// buffer is at capacity.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    pub fn write_entry(&self, entry: LogEntry) {
        let mut buf = self.entries.lock().expect("MemoryLogWriter lock poisoned");
        if buf.len() == self.max_entries {
            buf.pop_front();
        }
        buf.push_back(entry);
    }

    /// Convenience method that constructs a [`LogEntry`] with
    /// `Utc::now()` as the timestamp, then writes it.
    pub fn write_line(&self, stream: LogStream, message: &str, source: LogSource) {
        let entry = LogEntry {
            timestamp: Utc::now(),
            stream,
            message: message.to_string(),
            source,
            service: None,
            deployment: None,
        };
        self.write_entry(entry);
    }

    /// Returns a clone of all entries currently in the buffer, in
    /// chronological order (oldest first).
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn entries(&self) -> Vec<LogEntry> {
        self.entries
            .lock()
            .expect("MemoryLogWriter lock poisoned")
            .iter()
            .cloned()
            .collect()
    }

    /// Returns the last `n` entries (or fewer if the buffer holds less).
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn tail(&self, n: usize) -> Vec<LogEntry> {
        let buf = self.entries.lock().expect("MemoryLogWriter lock poisoned");
        buf.iter().rev().take(n).rev().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_entry_serialization_roundtrip() {
        let entry = LogEntry {
            timestamp: Utc::now(),
            stream: LogStream::Stdout,
            message: "hello world".to_string(),
            source: LogSource::Container("abc123".to_string()),
            service: Some("web".to_string()),
            deployment: None,
        };

        let json = serde_json::to_string(&entry).expect("serialize");
        let deserialized: LogEntry = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deserialized.message, "hello world");
        assert_eq!(deserialized.stream, LogStream::Stdout);
        assert_eq!(
            deserialized.source,
            LogSource::Container("abc123".to_string())
        );
        assert_eq!(deserialized.service, Some("web".to_string()));
        assert!(deserialized.deployment.is_none());
    }

    #[test]
    fn display_format_is_correct() {
        let entry = LogEntry {
            timestamp: Utc::now(),
            stream: LogStream::Stderr,
            message: "something failed".to_string(),
            source: LogSource::Daemon,
            service: None,
            deployment: None,
        };

        assert_eq!(entry.to_string(), "[stderr] something failed");

        let stdout_entry = LogEntry {
            stream: LogStream::Stdout,
            message: "ok".to_string(),
            ..entry
        };

        assert_eq!(stdout_entry.to_string(), "[stdout] ok");
    }

    #[test]
    fn log_query_default_is_empty() {
        let query = LogQuery::default();

        assert!(query.source.is_none());
        assert!(query.stream.is_none());
        assert!(query.since.is_none());
        assert!(query.until.is_none());
        assert!(query.tail.is_none());
    }

    #[test]
    fn file_log_writer_writes_jsonl() {
        let dir = std::env::temp_dir().join(format!("zlayer-log-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.jsonl");

        {
            let writer = FileLogWriter::new(&path).expect("open writer");
            assert_eq!(writer.path(), path);

            writer
                .write_line(
                    LogStream::Stdout,
                    "first line",
                    LogSource::Container("c1".into()),
                )
                .unwrap();
            writer
                .write_line(
                    LogStream::Stderr,
                    "second line",
                    LogSource::Job("j1".into()),
                )
                .unwrap();
        }

        let contents = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2);

        let first: LogEntry = serde_json::from_str(lines[0]).expect("parse first line");
        assert_eq!(first.message, "first line");
        assert_eq!(first.stream, LogStream::Stdout);
        assert_eq!(first.source, LogSource::Container("c1".into()));

        let second: LogEntry = serde_json::from_str(lines[1]).expect("parse second line");
        assert_eq!(second.message, "second line");
        assert_eq!(second.stream, LogStream::Stderr);
        assert_eq!(second.source, LogSource::Job("j1".into()));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_log_writer_appends_to_existing_file() {
        let dir =
            std::env::temp_dir().join(format!("zlayer-log-append-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("append.jsonl");

        {
            let writer = FileLogWriter::new(&path).unwrap();
            writer
                .write_line(LogStream::Stdout, "line 1", LogSource::Daemon)
                .unwrap();
        }
        {
            let writer = FileLogWriter::new(&path).unwrap();
            writer
                .write_line(LogStream::Stdout, "line 2", LogSource::Daemon)
                .unwrap();
        }

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents.lines().count(), 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn memory_log_writer_evicts_oldest() {
        let writer = MemoryLogWriter::new(3);

        for i in 0..5 {
            writer.write_line(LogStream::Stdout, &format!("msg {i}"), LogSource::Daemon);
        }

        let entries = writer.entries();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].message, "msg 2");
        assert_eq!(entries[1].message, "msg 3");
        assert_eq!(entries[2].message, "msg 4");
    }

    #[test]
    fn memory_log_writer_tail() {
        let writer = MemoryLogWriter::new(10);

        for i in 0..5 {
            writer.write_line(
                LogStream::Stdout,
                &format!("msg {i}"),
                LogSource::Build("b1".into()),
            );
        }

        let last2 = writer.tail(2);
        assert_eq!(last2.len(), 2);
        assert_eq!(last2[0].message, "msg 3");
        assert_eq!(last2[1].message, "msg 4");

        // Requesting more than available returns everything.
        let all = writer.tail(100);
        assert_eq!(all.len(), 5);
    }
}
