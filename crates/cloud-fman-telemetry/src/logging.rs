use tracing::Metadata;
use tracing_subscriber::filter::{FilterExt as _, filter_fn};
use tracing_subscriber::layer::Filter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

const OWNED_TARGET: &str = env!("CARGO_CRATE_NAME");

/// Initialize stderr diagnostics for only the collector target namespace.
pub fn init() {
    tracing_subscriber::registry()
        .with(fmt::layer().with_filter(configured_filter(EnvFilter::from_default_env())))
        .init();
}

fn configured_filter<S>(configured: EnvFilter) -> impl Filter<S> {
    filter_fn(is_owned).and(configured)
}

fn is_owned(metadata: &Metadata<'_>) -> bool {
    is_owned_target(metadata.target())
}

fn is_owned_target(target: &str) -> bool {
    target == OWNED_TARGET
        || target
            .strip_prefix(OWNED_TARGET)
            .is_some_and(|suffix| suffix.starts_with("::"))
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn permissive_filter_cannot_render_dependency_endpoint_or_capability() {
        const ENDPOINT: &str = "endpoint-id-that-must-not-render";
        const CAPABILITY: &str = "capability-that-must-not-render";

        let output = Arc::new(Mutex::new(Vec::new()));
        let writer = SharedWriter(output.clone());
        let layer = fmt::layer()
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .with_filter(configured_filter(EnvFilter::new("trace")));
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::debug_span!(
                target: "iroh::endpoint",
                "connect",
                remote = ENDPOINT,
                capability = CAPABILITY,
            );
            let _entered = span.enter();
            tracing::debug!(
                target: "iroh::_events::conn::connected",
                remote_id = ENDPOINT,
                capability = CAPABILITY,
                "connected"
            );
            tracing::warn!("owned polling diagnostic");
        });

        let rendered = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        assert!(rendered.contains("owned polling diagnostic"));
        assert!(!rendered.contains("connect"));
        assert!(!rendered.contains(ENDPOINT));
        assert!(!rendered.contains(CAPABILITY));
    }

    #[test]
    fn target_prefix_does_not_admit_lookalikes() {
        assert!(is_owned_target(OWNED_TARGET));
        assert!(is_owned_target(&format!("{OWNED_TARGET}::metrics_poller")));
        assert!(!is_owned_target(&format!("{OWNED_TARGET}_lookalike")));
        assert!(!is_owned_target("iroh"));
    }
}
