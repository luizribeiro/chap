use std::future::Future;
use tracing::{Instrument, debug, info_span};

pub(super) async fn trace_plugin_call<F, T, E>(
    plugin: &str,
    role: &'static str,
    function: &'static str,
    call: F,
) -> Result<T, E>
where
    F: Future<Output = Result<T, E>>,
{
    let span = info_span!("plugin_call", plugin = %plugin, role = %role, function = %function);
    let started = std::time::Instant::now();
    let result = call.instrument(span.clone()).await;
    debug!(
        parent: &span,
        elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0,
        outcome = if result.is_ok() { "ok" } else { "error" },
        "plugin call completed"
    );
    result
}
