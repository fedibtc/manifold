//! JSON tracing for explicitly shareable events.
//!
//! Formatting is delegated to `tracing_subscriber::fmt`. A per-layer filter
//! accepts only events carrying the typed field `safe_to_share = true`, while
//! `bounded-rolling-file` owns storage and retention.

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};

use bounded_rolling_file::{Config as RollingConfig, RollingFileAppender};
use tracing::field::{Field, Visit};
use tracing::{Event, Metadata};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::{Context, Filter};
use tracing_subscriber::{Layer, Registry};

/// Bounds queued journal memory to at most 8 MiB under the production event
/// limit, plus channel and allocation overhead.
const RECORD_QUEUE_CAPACITY: usize = 128;

/// Production limits for one process's safe-event journal.
#[derive(Clone, Copy, Debug)]
pub struct JournalConfig {
    /// Maximum size of one completed segment.
    pub max_segment_bytes: u64,
    /// Maximum number of segments retained, including the active segment.
    pub max_segments: usize,
    /// Maximum formatted size of one event. Larger events are dropped.
    pub max_event_bytes: usize,
}

impl Default for JournalConfig {
    fn default() -> Self {
        Self {
            max_segment_bytes: 5 * 1024 * 1024 / 2,
            max_segments: 2,
            max_event_bytes: 64 * 1024,
        }
    }
}

/// Type-erased layer ready to attach directly to a tracing registry.
pub type BoxedSafeEventLayer = Box<dyn Layer<Registry> + Send + Sync + 'static>;

/// Open a journal with the production retention policy.
pub fn layer(directory: impl Into<PathBuf>) -> io::Result<BoxedSafeEventLayer> {
    layer_with_config(directory, JournalConfig::default())
}

/// Open a journal with explicit limits.
pub fn layer_with_config(
    directory: impl Into<PathBuf>,
    config: JournalConfig,
) -> io::Result<BoxedSafeEventLayer> {
    validate_config(config)?;
    let appender = RollingFileAppender::open(
        directory,
        RollingConfig {
            max_file_bytes: config.max_segment_bytes,
            max_files: config.max_segments,
        },
    )?;
    let (sender, receiver) = mpsc::sync_channel(RECORD_QUEUE_CAPACITY);
    let thread = thread::Builder::new()
        .name("safe-event-writer".to_owned())
        .spawn(move || write_records(appender, receiver))?;
    let writer = JournalMakeWriter {
        worker: Arc::new(JournalWorker {
            sender: Some(sender),
            thread: Some(thread),
        }),
        max_event_bytes: config.max_event_bytes,
    };
    let layer = tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .with_writer(writer)
        .with_filter(SafeToShare);
    Ok(Box::new(layer))
}

fn write_records(mut appender: RollingFileAppender, receiver: Receiver<Vec<u8>>) {
    let mut enabled = true;
    for record in receiver {
        if enabled && appender.append_record(&record).is_err() {
            // Keep draining the bounded queue and retain the appender so its
            // single-writer lock remains held for this layer's lifetime.
            enabled = false;
        }
    }
}

fn validate_config(config: JournalConfig) -> io::Result<()> {
    if config.max_segment_bytes == 0 || config.max_segments == 0 || config.max_event_bytes == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "safe-event journal limits must be nonzero",
        ));
    }
    if config.max_event_bytes as u64 > config.max_segment_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "safe-event maximum event size exceeds segment size",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct SafeToShare;

impl<S> Filter<S> for SafeToShare {
    fn enabled(&self, metadata: &Metadata<'_>, _context: &Context<'_, S>) -> bool {
        metadata.is_event() && metadata.fields().field("safe_to_share").is_some()
    }

    fn callsite_enabled(
        &self,
        metadata: &'static Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        if metadata.is_event() && metadata.fields().field("safe_to_share").is_some() {
            tracing::subscriber::Interest::sometimes()
        } else {
            tracing::subscriber::Interest::never()
        }
    }

    fn event_enabled(&self, event: &Event<'_>, _context: &Context<'_, S>) -> bool {
        let mut marker = MarkerVisitor(false);
        event.record(&mut marker);
        marker.0
    }
}

struct MarkerVisitor(bool);

impl Visit for MarkerVisitor {
    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() == "safe_to_share" {
            self.0 = value;
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
}

#[derive(Clone)]
struct JournalMakeWriter {
    worker: Arc<JournalWorker>,
    max_event_bytes: usize,
}

struct JournalWorker {
    sender: Option<SyncSender<Vec<u8>>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for JournalWorker {
    fn drop(&mut self) {
        // Disconnect first so the worker drains the finite queue and exits.
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl<'a> MakeWriter<'a> for JournalMakeWriter {
    type Writer = EventWriter;

    fn make_writer(&'a self) -> Self::Writer {
        EventWriter {
            worker: Arc::clone(&self.worker),
            bytes: Vec::new(),
            max_event_bytes: self.max_event_bytes,
            overflowed: false,
        }
    }
}

/// Buffers one normal fmt-layer emission for validation and size enforcement.
struct EventWriter {
    worker: Arc<JournalWorker>,
    bytes: Vec<u8>,
    max_event_bytes: usize,
    overflowed: bool,
}

impl Write for EventWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|len| len > self.max_event_bytes)
        {
            self.overflowed = true;
            self.bytes.clear();
        } else if !self.overflowed {
            self.bytes.extend_from_slice(bytes);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for EventWriter {
    fn drop(&mut self) {
        if self.overflowed || self.bytes.is_empty() {
            return;
        }
        if !self.bytes.ends_with(b"\n")
            || serde_json::from_slice::<serde_json::Value>(&self.bytes).is_err()
        {
            return;
        }
        if let Some(sender) = &self.worker.sender {
            // Diagnostics must never wait for storage. A full queue or dead
            // worker drops this event; both cases leave guardian work moving.
            let _ = sender.try_send(std::mem::take(&mut self.bytes));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::{BufRead as _, BufReader};
    use std::path::Path;
    use std::sync::Barrier;
    use std::time::Duration;

    use serde_json::Value;
    use tracing_subscriber::prelude::*;

    use super::*;

    fn segment_paths(directory: &Path) -> Vec<PathBuf> {
        let mut paths = std::fs::read_dir(directory)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("events-") && name.ends_with(".jsonl"))
            })
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }

    #[test]
    fn production_journal_retains_two_equal_segments_within_five_mib() {
        let config = JournalConfig::default();

        assert_eq!(config.max_segment_bytes, 5 * 1024 * 1024 / 2);
        assert_eq!(config.max_segments, 2);
        assert_eq!(
            config.max_segment_bytes * config.max_segments as u64,
            5 * 1024 * 1024
        );
    }

    fn read_all(directory: &Path) -> Vec<Value> {
        let mut events = Vec::new();
        for path in segment_paths(directory) {
            let file = File::open(path).unwrap();
            for line in BufReader::new(file).lines() {
                events.push(serde_json::from_str(&line.unwrap()).unwrap());
            }
        }
        events
    }

    fn wait_for_segment(directory: &Path) -> PathBuf {
        for _ in 0..100 {
            if let Some(path) = segment_paths(directory).pop() {
                return path;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("journal worker did not create a segment");
    }

    #[test]
    fn stores_only_typed_true_without_span_fields() {
        let directory = tempfile::tempdir().unwrap();
        let subscriber = tracing_subscriber::registry().with(layer(directory.path()).unwrap());

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("private", secret = "must-not-be-inherited");
            let _guard = span.enter();
            tracing::info!(ordinary = 1, "not stored");
            tracing::info!(safe_to_share = false, ordinary = 2, "not stored either");
            tracing::info!(
                safe_to_share = "true",
                ordinary = 3,
                "string is not authority"
            );
            tracing::info!(safe_to_share = true, answer = 42, "stored");
        });

        let events = read_all(directory.path());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["fields"]["answer"], 42);
        assert_eq!(events[0]["fields"]["message"], "stored");
        assert!(events[0].get("span").is_none());
        assert!(!events[0].to_string().contains("must-not-be-inherited"));
    }

    #[test]
    fn drops_an_oversized_event() {
        let directory = tempfile::tempdir().unwrap();
        let config = JournalConfig {
            max_segment_bytes: 256,
            max_segments: 2,
            max_event_bytes: 256,
        };
        let subscriber = tracing_subscriber::registry()
            .with(layer_with_config(directory.path(), config).unwrap());
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(safe_to_share = true, value = "x".repeat(1024), "too large");
        });

        assert!(read_all(directory.path()).is_empty());
        assert!(segment_paths(directory.path()).is_empty());
    }

    #[test]
    fn concurrent_events_remain_complete_json_lines() {
        let directory = tempfile::tempdir().unwrap();
        let subscriber = tracing_subscriber::registry().with(layer(directory.path()).unwrap());
        let dispatch = tracing::Dispatch::new(subscriber);
        std::thread::scope(|scope| {
            for worker in 0..4 {
                let dispatch = dispatch.clone();
                scope.spawn(move || {
                    tracing::dispatcher::with_default(&dispatch, || {
                        for event in 0..16 {
                            tracing::info!(safe_to_share = true, worker, event, "concurrent");
                        }
                    });
                });
            }
        });
        drop(dispatch);

        assert_eq!(read_all(directory.path()).len(), 64);
    }

    #[test]
    fn full_stalled_queue_never_blocks_event_emitter() {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(b"already full".to_vec()).unwrap();
        let gate = Arc::new(Barrier::new(2));
        let worker_gate = Arc::clone(&gate);
        let thread = std::thread::spawn(move || {
            worker_gate.wait();
            drop(receiver);
        });
        let worker = Arc::new(JournalWorker {
            sender: Some(sender),
            thread: Some(thread),
        });
        let event_worker = Arc::clone(&worker);
        let (done_tx, done_rx) = mpsc::channel();
        let emitter = std::thread::spawn(move || {
            drop(EventWriter {
                worker: event_worker,
                bytes: b"{}\n".to_vec(),
                max_event_bytes: 3,
                overflowed: false,
            });
            done_tx.send(()).unwrap();
        });

        let completed_without_waiting = done_rx.recv_timeout(Duration::from_secs(1)).is_ok();
        gate.wait();
        emitter.join().unwrap();
        drop(worker);
        assert!(
            completed_without_waiting,
            "a full stalled queue blocked the event emitter"
        );
    }

    #[test]
    fn drops_a_stale_fmt_prefix_after_a_caught_field_panic() {
        struct Panics;

        impl std::fmt::Debug for Panics {
            fn fmt(&self, _formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                panic!("field formatting panic");
            }
        }

        let directory = tempfile::tempdir().unwrap();
        let subscriber = tracing_subscriber::registry().with(layer(directory.path()).unwrap());
        tracing::subscriber::with_default(subscriber, || {
            let result = std::panic::catch_unwind(|| {
                tracing::info!(safe_to_share = true, value = ?Panics, "panics");
            });
            assert!(result.is_err());
            tracing::info!(safe_to_share = true, "recovery");
            tracing::info!(safe_to_share = true, "stored after recovery");
        });

        let events = read_all(directory.path());
        assert!(
            events
                .iter()
                .all(|event| event["fields"]["message"] != "panics")
        );
        assert_eq!(
            events.last().unwrap()["fields"]["message"],
            "stored after recovery"
        );
    }

    #[test]
    fn boxed_layer_composes_before_a_normal_formatting_layer() {
        use tracing_subscriber::filter::LevelFilter;

        let directory = tempfile::tempdir().unwrap();
        let safe_events = Some(layer(directory.path()).unwrap());
        let stderr = tracing_subscriber::fmt::layer().with_filter(LevelFilter::INFO);
        let _subscriber = tracing_subscriber::registry()
            .with(safe_events)
            .with(stderr);
    }

    #[test]
    fn disabled_sink_holds_the_writer_lock_until_the_layer_drops() {
        let directory = tempfile::tempdir().unwrap();
        let config = JournalConfig {
            max_segment_bytes: 512,
            max_segments: 1,
            max_event_bytes: 512,
        };
        let safe_events = layer_with_config(directory.path(), config).unwrap();
        let subscriber = tracing_subscriber::registry().with(safe_events);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(safe_to_share = true, value = "x".repeat(250), "first");
            let path = wait_for_segment(directory.path());
            std::fs::remove_file(&path).unwrap();
            std::fs::create_dir(&path).unwrap();
            tracing::info!(
                safe_to_share = true,
                value = "x".repeat(250),
                "rotation fails"
            );

            let Err(error) = layer_with_config(directory.path(), config) else {
                panic!("disabled layer must retain the writer lock");
            };
            assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        });

        std::fs::remove_dir(segment_paths(directory.path()).pop().unwrap()).unwrap();
        layer_with_config(directory.path(), config).unwrap();
    }
}
