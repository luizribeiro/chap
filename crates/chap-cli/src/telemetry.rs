use crate::tui::DiagnosticWriter;
use std::{
    env,
    io::{self, IsTerminal, Write},
    sync::{Arc, Mutex},
};
use tracing::{Level, Metadata};
use tracing_subscriber::{EnvFilter, fmt::MakeWriter};

const DEFAULT_FILTER: &str = "warn,chap=info";
type WriterFactory = dyn Fn() -> Box<dyn Write + Send> + Send + Sync;

#[derive(Clone)]
pub(crate) struct TracingRouter {
    active: Arc<Mutex<Option<DiagnosticWriter>>>,
    stderr: Arc<WriterFactory>,
}

impl TracingRouter {
    pub(crate) fn new() -> Self {
        Self {
            active: Arc::new(Mutex::new(None)),
            stderr: Arc::new(|| Box::new(io::stderr())),
        }
    }

    #[cfg(test)]
    fn with_stderr<W>(make_writer: impl Fn() -> W + Send + Sync + 'static) -> Self
    where
        W: Write + Send + 'static,
    {
        Self {
            active: Arc::new(Mutex::new(None)),
            stderr: Arc::new(move || Box::new(make_writer())),
        }
    }

    pub(crate) fn route_to(&self, writer: DiagnosticWriter) -> TuiTracingGuard {
        let previous = self.active().replace(writer);
        debug_assert!(previous.is_none(), "TUI tracing route was already active");
        TuiTracingGuard {
            router: self.clone(),
        }
    }

    fn active(&self) -> std::sync::MutexGuard<'_, Option<DiagnosticWriter>> {
        self.active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn writer_for(&self, metadata: Option<&Metadata<'_>>) -> RoutedWriter {
        if let Some(writer) = self.active().clone() {
            return RoutedWriter::Diagnostics(writer);
        }
        if metadata.is_none_or(batch_level_is_visible) {
            return RoutedWriter::Stderr((self.stderr)());
        }
        RoutedWriter::Discard(io::sink())
    }
}

impl<'writer> MakeWriter<'writer> for TracingRouter {
    type Writer = RoutedWriter;

    fn make_writer(&'writer self) -> Self::Writer {
        self.writer_for(None)
    }

    fn make_writer_for(&'writer self, metadata: &Metadata<'_>) -> Self::Writer {
        self.writer_for(Some(metadata))
    }
}

pub(crate) struct TuiTracingGuard {
    router: TracingRouter,
}

impl Drop for TuiTracingGuard {
    fn drop(&mut self) {
        self.router.active().take();
    }
}

pub(crate) enum RoutedWriter {
    Diagnostics(DiagnosticWriter),
    Stderr(Box<dyn Write + Send>),
    Discard(io::Sink),
}

impl Write for RoutedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self {
            Self::Diagnostics(writer) => writer.write(buffer),
            Self::Stderr(writer) => writer.write(buffer),
            Self::Discard(writer) => writer.write(buffer),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Diagnostics(writer) => writer.flush(),
            Self::Stderr(writer) => writer.flush(),
            Self::Discard(writer) => writer.flush(),
        }
    }
}

pub(crate) fn install(router: TracingRouter) {
    tracing::subscriber::set_global_default(subscriber(
        router,
        filter_from_env(),
        io::stderr().is_terminal(),
    ))
    .expect("tracing subscriber must be installed only once");
}

fn subscriber(
    router: TracingRouter,
    filter: EnvFilter,
    ansi: bool,
) -> impl tracing::Subscriber + Send + Sync {
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(ansi)
        .with_writer(router)
        .finish()
}

fn filter_from_env() -> EnvFilter {
    env::var("RUST_LOG")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .and_then(|value| EnvFilter::try_new(value).ok())
        .unwrap_or_else(|| EnvFilter::new(DEFAULT_FILTER))
}

fn batch_level_is_visible(metadata: &Metadata<'_>) -> bool {
    matches!(*metadata.level(), Level::ERROR | Level::WARN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn tui_subscriber_routes_events_to_the_diagnostic_sink_not_stderr() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let router = TracingRouter::with_stderr({
            let stderr = Arc::clone(&stderr);
            move || Captured(Arc::clone(&stderr))
        });
        let _route = router.route_to(DiagnosticWriter::new(Captured(Arc::clone(&log))));
        let subscriber = subscriber(router, EnvFilter::new("trace"), false);

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "chap_core::test", plugin = "example", "test event");
        });

        let log = String::from_utf8(log.lock().unwrap().clone()).unwrap();
        assert!(log.contains("test event"), "{log}");
        assert!(log.contains("plugin=\"example\""), "{log}");
        assert!(stderr.lock().unwrap().is_empty());
    }
}
