use crate::render::render_start_error;
use chap_core::AgentBuilder;

pub(super) async fn plugin_check(builder: AgentBuilder) -> Result<(), String> {
    let plugin_count = builder.plugins().count();
    let agent = builder.start().await.map_err(render_start_error)?;
    tokio::task::spawn_blocking(move || drop(agent))
        .await
        .map_err(|error| format!("failed to clean up plugin host: {error}"))?;
    println!("Loaded {plugin_count} plugin(s).");
    Ok(())
}
