//! WAL file change detection
//!
//! Monitors SQLite WAL files for changes and parses WAL headers to track frame counts.

use crate::error::{LayerStorageError, Result};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::trace;

/// WAL file change event
#[derive(Debug, Clone)]
pub struct WalEvent {
    /// Current frame count in the WAL
    pub frame_count: u64,
    /// Size of the WAL file in bytes
    pub file_size: u64,
    /// Whether this is a checkpoint event
    pub is_checkpoint: bool,
}

/// SQLite WAL header structure (first 32 bytes)
///
/// See: https://www.sqlite.org/fileformat2.html#walformat
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct WalHeader {
    /// Magic number: 0x377f0682 or 0x377f0683
    magic: u32,
    /// Format version (currently 3007000)
    format_version: u32,
    /// Database page size
    page_size: u32,
    /// Checkpoint sequence number
    checkpoint_seq: u32,
    /// Salt-1 (random value)
    salt1: u32,
    /// Salt-2 (random value)
    salt2: u32,
    /// Checksum-1
    checksum1: u32,
    /// Checksum-2
    checksum2: u32,
}

impl WalHeader {
    /// Parse WAL header from bytes
    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 32 {
            return None;
        }

        // WAL header is big-endian
        let magic = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);

        // Validate magic number
        if magic != 0x377f_0682 && magic != 0x377f_0683 {
            return None;
        }

        Some(Self {
            magic,
            format_version: u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            page_size: u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            checkpoint_seq: u32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            salt1: u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
            salt2: u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
            checksum1: u32::from_be_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
            checksum2: u32::from_be_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]),
        })
    }
}

/// WAL frame header (24 bytes per frame)
///
/// Reserved for future frame-level parsing
#[repr(C)]
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
struct WalFrameHeader {
    /// Page number
    page_number: u32,
    /// Database size in pages (for commit frames) or 0
    db_size: u32,
    /// Salt-1 (must match WAL header)
    salt1: u32,
    /// Salt-2 (must match WAL header)
    salt2: u32,
    /// Checksum-1
    checksum1: u32,
    /// Checksum-2
    checksum2: u32,
}

/// Monitors a SQLite WAL file for changes
pub struct WalMonitor {
    /// Path to the WAL file
    wal_path: PathBuf,
    /// Last known frame count
    last_frame_count: Arc<AtomicU64>,
    /// Last known file size
    last_file_size: Arc<AtomicU64>,
    /// File watcher
    _watcher: RecommendedWatcher,
    /// Event receiver
    event_rx: mpsc::Receiver<notify::Result<Event>>,
}

impl WalMonitor {
    /// Create a new WAL monitor
    ///
    /// # Arguments
    ///
    /// * `wal_path` - Path to the SQLite WAL file
    ///
    /// # Errors
    ///
    /// Returns an error if the file watcher cannot be initialized.
    pub fn new(wal_path: PathBuf) -> Result<Self> {
        let (tx, rx) = mpsc::channel(100);

        // Create watcher with a small debounce
        let watcher = notify::recommended_watcher(move |res| {
            let _ = tx.blocking_send(res);
        })
        .map_err(|e| LayerStorageError::Io(std::io::Error::other(format!("Watcher error: {e}"))))?;

        // Get initial state if file exists
        let (initial_size, initial_frames) = if wal_path.exists() {
            let metadata = std::fs::metadata(&wal_path)?;
            let size = metadata.len();
            let frames = Self::count_frames_from_file(&wal_path).unwrap_or(0);
            (size, frames)
        } else {
            (0, 0)
        };

        let mut monitor = Self {
            wal_path,
            last_frame_count: Arc::new(AtomicU64::new(initial_frames)),
            last_file_size: Arc::new(AtomicU64::new(initial_size)),
            _watcher: watcher,
            event_rx: rx,
        };

        // Start watching the parent directory
        monitor.start_watching()?;

        Ok(monitor)
    }

    /// Start watching the WAL file
    fn start_watching(&mut self) -> Result<()> {
        // Watch the parent directory since the WAL file may be created/deleted
        if let Some(parent) = self.wal_path.parent() {
            if parent.exists() {
                // We need to re-create the watcher since we can't modify it after creation
                // The watcher is already watching from the constructor
                let (tx, rx) = mpsc::channel(100);
                let wal_path = self.wal_path.clone();

                let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
                    // Filter to only WAL file events
                    if let Ok(event) = &res {
                        let is_wal_event = event.paths.iter().any(|p| {
                            p.file_name()
                                .map(|n| n.to_string_lossy().ends_with("-wal"))
                                .unwrap_or(false)
                                || p == &wal_path
                        });
                        if is_wal_event {
                            let _ = tx.blocking_send(res);
                        }
                    }
                })
                .map_err(|e| {
                    LayerStorageError::Io(std::io::Error::other(format!("Watcher error: {e}")))
                })?;

                watcher
                    .watch(parent, RecursiveMode::NonRecursive)
                    .map_err(|e| {
                        LayerStorageError::Io(std::io::Error::other(format!("Watch error: {e}")))
                    })?;

                self._watcher = watcher;
                self.event_rx = rx;
            }
        }
        Ok(())
    }

    /// Check for WAL changes
    ///
    /// Returns `Some(WalEvent)` if changes were detected, `None` otherwise.
    pub async fn check_for_changes(&self) -> Result<Option<WalEvent>> {
        // Check if file exists and get current state
        if !self.wal_path.exists() {
            // WAL might have been checkpointed away
            let last_size = self.last_file_size.load(Ordering::SeqCst);
            if last_size > 0 {
                // WAL was deleted (checkpoint completed)
                self.last_file_size.store(0, Ordering::SeqCst);
                self.last_frame_count.store(0, Ordering::SeqCst);
                return Ok(Some(WalEvent {
                    frame_count: 0,
                    file_size: 0,
                    is_checkpoint: true,
                }));
            }
            return Ok(None);
        }

        // Get current file size
        let metadata = tokio::fs::metadata(&self.wal_path).await?;
        let current_size = metadata.len();
        let last_size = self.last_file_size.load(Ordering::SeqCst);

        if current_size == last_size {
            return Ok(None);
        }

        // Size changed, count frames
        let frame_count = Self::count_frames_from_file(&self.wal_path).unwrap_or(0);
        let last_frames = self.last_frame_count.load(Ordering::SeqCst);

        // Update state
        self.last_file_size.store(current_size, Ordering::SeqCst);
        self.last_frame_count.store(frame_count, Ordering::SeqCst);

        // If size decreased, it's likely a checkpoint
        let is_checkpoint = current_size < last_size;

        if frame_count != last_frames || is_checkpoint {
            trace!(
                "WAL change: {} -> {} frames, {} -> {} bytes",
                last_frames,
                frame_count,
                last_size,
                current_size
            );

            Ok(Some(WalEvent {
                frame_count,
                file_size: current_size,
                is_checkpoint,
            }))
        } else {
            Ok(None)
        }
    }

    /// Count frames in a WAL file
    fn count_frames_from_file(path: &PathBuf) -> Option<u64> {
        let data = std::fs::read(path).ok()?;
        Self::count_frames(&data)
    }

    /// Count frames in WAL data
    fn count_frames(data: &[u8]) -> Option<u64> {
        if data.len() < 32 {
            return Some(0);
        }

        let header = WalHeader::from_bytes(data)?;
        let page_size = header.page_size as usize;

        if page_size == 0 {
            return Some(0);
        }

        // Frame size = 24 byte header + page_size
        let frame_size = 24 + page_size;

        // Calculate number of frames
        let data_after_header = data.len().saturating_sub(32);
        let frame_count = data_after_header / frame_size;

        Some(frame_count as u64)
    }

    /// Get the current frame count
    #[allow(dead_code)]
    pub fn frame_count(&self) -> u64 {
        self.last_frame_count.load(Ordering::SeqCst)
    }

    /// Get the current WAL file size
    #[allow(dead_code)]
    pub fn file_size(&self) -> u64 {
        self.last_file_size.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wal_header_parsing() {
        // Valid WAL header (big-endian)
        let mut header_bytes = vec![0u8; 32];
        // Magic number 0x377f0682
        header_bytes[0..4].copy_from_slice(&0x377f_0682_u32.to_be_bytes());
        // Format version 3007000
        header_bytes[4..8].copy_from_slice(&3_007_000_u32.to_be_bytes());
        // Page size 4096
        header_bytes[8..12].copy_from_slice(&4096_u32.to_be_bytes());

        let header = WalHeader::from_bytes(&header_bytes).unwrap();
        assert_eq!(header.magic, 0x377f_0682);
        assert_eq!(header.format_version, 3_007_000);
        assert_eq!(header.page_size, 4096);
    }

    #[test]
    fn test_invalid_wal_header() {
        // Invalid magic number
        let header_bytes = vec![0u8; 32];
        assert!(WalHeader::from_bytes(&header_bytes).is_none());

        // Too short
        let short_bytes = vec![0u8; 16];
        assert!(WalHeader::from_bytes(&short_bytes).is_none());
    }

    #[test]
    fn test_frame_counting() {
        // Create a valid WAL with header and 2 frames
        let page_size: usize = 4096;
        let frame_size = 24 + page_size;
        let mut data = vec![0u8; 32 + 2 * frame_size];

        // Write valid header
        data[0..4].copy_from_slice(&0x377f_0682_u32.to_be_bytes());
        data[8..12].copy_from_slice(&(page_size as u32).to_be_bytes());

        let count = WalMonitor::count_frames(&data).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_empty_wal() {
        let data = vec![0u8; 16];
        let count = WalMonitor::count_frames(&data);
        assert_eq!(count, Some(0));
    }
}
