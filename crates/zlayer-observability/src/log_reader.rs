//! Log reader implementations for structured and legacy log files.
//!
//! Provides [`FileLogReader`] for reading JSONL-formatted log files and
//! legacy plain-text stdout/stderr logs, plus [`apply_query`] for filtering
//! [`LogEntry`] vectors against a [`LogQuery`].

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use chrono::{DateTime, Utc};

use crate::logs::{LogEntry, LogQuery, LogSource, LogStream};

/// Reads structured log entries from JSONL files on disk.
///
/// All methods are static — `FileLogReader` carries no state and exists
/// only as a namespace for related functions.
pub struct FileLogReader;

impl FileLogReader {
    /// Read a JSONL log file and return entries matching the given query.
    ///
    /// Each line in the file is expected to be a JSON-serialized [`LogEntry`].
    /// Lines that fail to deserialize are silently skipped.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the file cannot be opened or read.
    pub fn read(path: &Path, query: &LogQuery) -> Result<Vec<LogEntry>, std::io::Error> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut entries = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<LogEntry>(&line) {
                entries.push(entry);
            }
        }

        Ok(apply_query(entries, query))
    }

    /// Read legacy separate stdout.log / stderr.log files.
    ///
    /// Old-format logs are plain text with no timestamps or structured
    /// metadata. Each line becomes a [`LogEntry`] with:
    /// - `timestamp` set to [`DateTime::UNIX_EPOCH`](chrono::DateTime) (epoch zero)
    /// - `stream` set to [`LogStream::Stdout`] or [`LogStream::Stderr`]
    /// - `source` set to [`LogSource::Daemon`]
    /// - `service` and `deployment` set to `None`
    ///
    /// Entries from both files are interleaved (all stdout first, then all
    /// stderr) and then filtered with [`apply_query`].
    ///
    /// # Errors
    ///
    /// Returns an I/O error if either file cannot be opened or read.
    pub fn read_legacy(
        stdout_path: &Path,
        stderr_path: &Path,
        query: &LogQuery,
    ) -> Result<Vec<LogEntry>, std::io::Error> {
        let mut entries = Vec::new();

        let read_plain = |path: &Path,
                          stream: LogStream,
                          entries: &mut Vec<LogEntry>|
         -> Result<(), std::io::Error> {
            let file = File::open(path)?;
            let reader = BufReader::new(file);
            for line in reader.lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                entries.push(LogEntry {
                    timestamp: DateTime::<Utc>::UNIX_EPOCH,
                    stream,
                    message: line,
                    source: LogSource::Daemon,
                    service: None,
                    deployment: None,
                });
            }
            Ok(())
        };

        read_plain(stdout_path, LogStream::Stdout, &mut entries)?;
        read_plain(stderr_path, LogStream::Stderr, &mut entries)?;

        Ok(apply_query(entries, query))
    }
}

/// Apply query filters to a vec of log entries.
///
/// Filters are applied in order: `stream`, `source`, `since`, `until`.
/// After filtering, if `query.tail` is set, only the last N entries are
/// returned.
#[must_use]
pub fn apply_query(entries: Vec<LogEntry>, query: &LogQuery) -> Vec<LogEntry> {
    let mut filtered: Vec<LogEntry> = entries
        .into_iter()
        .filter(|e| {
            if let Some(ref stream) = query.stream {
                if &e.stream != stream {
                    return false;
                }
            }
            if let Some(ref since) = query.since {
                if &e.timestamp < since {
                    return false;
                }
            }
            if let Some(ref until) = query.until {
                if &e.timestamp > until {
                    return false;
                }
            }
            if let Some(ref source) = query.source {
                if &e.source != source {
                    return false;
                }
            }
            true
        })
        .collect();

    if let Some(tail) = query.tail {
        let skip = filtered.len().saturating_sub(tail);
        filtered = filtered.into_iter().skip(skip).collect();
    }

    filtered
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Helper: write lines to a temporary file and return its path.
    fn write_temp_file(name: &str, content: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        // Include thread id and a suffix to avoid collisions with parallel tests.
        path.push(format!(
            "zlayer_log_reader_test_{}_{}",
            name,
            std::process::id()
        ));
        let mut f = File::create(&path).expect("create temp file");
        f.write_all(content.as_bytes()).expect("write temp file");
        path
    }

    fn make_entry(stream: LogStream, msg: &str, ts: DateTime<Utc>) -> LogEntry {
        LogEntry {
            timestamp: ts,
            stream,
            message: msg.to_string(),
            source: LogSource::Container("test".to_string()),
            service: None,
            deployment: None,
        }
    }

    // -----------------------------------------------------------
    // FileLogReader::read — JSONL filtering by stream
    // -----------------------------------------------------------
    #[test]
    fn read_jsonl_filters_by_stream() {
        let ts = Utc::now();
        let e1 = make_entry(LogStream::Stdout, "hello", ts);
        let e2 = make_entry(LogStream::Stderr, "error", ts);
        let e3 = make_entry(LogStream::Stdout, "world", ts);

        let content = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&e1).unwrap(),
            serde_json::to_string(&e2).unwrap(),
            serde_json::to_string(&e3).unwrap(),
        );

        let path = write_temp_file("jsonl_stream", &content);
        let query = LogQuery {
            stream: Some(LogStream::Stdout),
            ..Default::default()
        };

        let results = FileLogReader::read(&path, &query).expect("read");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].message, "hello");
        assert_eq!(results[1].message, "world");

        std::fs::remove_file(&path).ok();
    }

    // -----------------------------------------------------------
    // FileLogReader::read_legacy — plain text stdout / stderr
    // -----------------------------------------------------------
    #[test]
    fn read_legacy_stdout_stderr() {
        let stdout = write_temp_file("legacy_stdout", "line one\nline two\n");
        let stderr = write_temp_file("legacy_stderr", "err line\n");

        let query = LogQuery::default();
        let results = FileLogReader::read_legacy(&stdout, &stderr, &query).expect("read_legacy");

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].stream, LogStream::Stdout);
        assert_eq!(results[0].message, "line one");
        assert_eq!(results[1].stream, LogStream::Stdout);
        assert_eq!(results[1].message, "line two");
        assert_eq!(results[2].stream, LogStream::Stderr);
        assert_eq!(results[2].message, "err line");

        // All timestamps should be UNIX_EPOCH because legacy format has none.
        for e in &results {
            assert_eq!(e.timestamp, DateTime::<Utc>::UNIX_EPOCH);
        }

        std::fs::remove_file(&stdout).ok();
        std::fs::remove_file(&stderr).ok();
    }

    // -----------------------------------------------------------
    // apply_query — tail limit
    // -----------------------------------------------------------
    #[test]
    fn apply_query_tail_limit() {
        let ts = Utc::now();
        let entries: Vec<LogEntry> = (0..10)
            .map(|i| make_entry(LogStream::Stdout, &format!("msg {i}"), ts))
            .collect();

        let query = LogQuery {
            tail: Some(3),
            ..Default::default()
        };

        let results = apply_query(entries, &query);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].message, "msg 7");
        assert_eq!(results[1].message, "msg 8");
        assert_eq!(results[2].message, "msg 9");
    }

    // -----------------------------------------------------------
    // apply_query — since / until window
    // -----------------------------------------------------------
    #[test]
    fn apply_query_since_until() {
        use chrono::Duration;

        let now = Utc::now();
        let entries = vec![
            make_entry(LogStream::Stdout, "old", now - Duration::hours(3)),
            make_entry(LogStream::Stdout, "recent", now - Duration::hours(1)),
            make_entry(LogStream::Stdout, "future", now + Duration::hours(1)),
        ];

        let query = LogQuery {
            since: Some(now - Duration::hours(2)),
            until: Some(now),
            ..Default::default()
        };

        let results = apply_query(entries, &query);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].message, "recent");
    }
}
