use std::error::Error;

use lockgate::{HostBuilder, InvocationCtx, PluginConfig, RuntimeLimits};

const STARTUP_FUEL: u64 = 1_000_000;
const CALL_FUEL: u64 = 100_000_000;

lockgate::host_bindings!({
    path: "../../plugins/openai-compatible/wit",
    world: "provider-plugin",
});

pub async fn complete(
    wasm: &[u8],
    settings: serde_json::Value,
    request: provider::CompletionRequest,
) -> Result<provider::Completion, Box<dyn Error>> {
    let mut builder = HostBuilder::new(())?;
    let prepared = builder
        .prepare(
            "openai",
            wasm,
            PluginConfig {
                settings: Some(settings),
                ..PluginConfig::default()
            },
        )
        .await?;
    let acceptance = prepared.accept_all();
    let handle = builder
        .admit(
            prepared,
            acceptance,
            RuntimeLimits::default(),
            InvocationCtx::bounded(STARTUP_FUEL),
        )
        .await?;
    let host = builder.finish();
    let client = host.client::<provider::Role>(&handle)?;
    let completion = client
        .complete(InvocationCtx::bounded(CALL_FUEL), request)
        .await?
        .map_err(std::io::Error::other)?;

    Ok(completion)
}
