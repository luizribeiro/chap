# SAGE

SAGE (Sandboxed Agent with Guarded Extensions) is a coding agent with a headless
core, user-facing frontends, and capability-scoped WebAssembly plugins provided
by Lockgate.

## Workspace

- `crates/sage-core` owns configuration, plugin loading, and the headless agent
  runtime.
- `crates/sage-cli` builds the `sage` executable and owns command-line and
  terminal interaction.
- `crates/sage-plugin` is the thin plugin-author facade over Lockgate and owns
  the shared `chap:agent` WIT package.
- `plugins` contains independently compiled WebAssembly components.

The CLI is the default workspace member, so root-level `cargo run` commands keep
working while other frontends can depend directly on `sage-core`.

Running SAGE without a subcommand opens its terminal interface using the
configured `openai` provider:

```console
cargo run
```

## Plugins

Plugins and their configuration live in `sage.toml`. List the configured
plugins with:

```console
cargo run -- plugins list
```

Use `--config /path/to/sage.toml` to read a different file.

The repository includes an OpenAI-compatible Chat Completions provider and Kagi
web tools. Build their configured release components with the system Cargo:

```console
cargo build -p sage-openai-compatible -p sage-kagi --release --target wasm32-wasip2
```

Each plugin is keyed by an operator-assigned instance id and maps directly to
its component and settings:

```toml
[plugins.openai]
component = "./target/wasm32-wasip2/release/sage_openai_compatible.wasm"

[plugins.openai.settings]
base-url = "http://127.0.0.1:8080/v1"
egress-origin = "http://127.0.0.1:8080"
model = "example-model"
# Optional: the host reads this environment variable without storing its value
# in sage.toml.
api-key-env = "OPENAI_API_KEY"
```

Each built-in plugin carries its own Cargo configuration and defaults to the
`wasm32-wasip2` target. The development shell provides Wasmtime as Cargo's test
runner, so `cargo test`, `cargo check`, and `cargo clippy` work directly from a
plugin directory without target flags.

From the workspace root, run the complete native and WASI test suites with:

```console
cargo test-all
```

The OpenAI-compatible plugin declares network egress through Lockgate and
resolves its exact origin from `egress-origin`. The scheme and effective port
are part of the origin. Kagi declares the literal origin `https://kagi.com` and
therefore needs no configurable origin.

### Permission grants

Lockgate v2 admits a component only after its exact resolved permission
manifest has been approved. Review all configured instances, approve each one,
then check that the components can be admitted:

```console
cargo run -- grants review
cargo run -- grants approve openai
cargo run -- grants approve kagi
cargo run -- plugins check
```

`grants review` also accepts one instance id. `grants deny <instance-id>` removes
that instance's approval. SAGE stores approvals in `consent.json` beside the
selected `sage.toml`; concrete scopes remain in `sage.toml`. A permission
expansion, such as changing `egress-origin`, blocks admission until the new
manifest is reviewed and approved. Narrowing or removing authority is reported
as non-blocking drift.

`plugins check` verifies that every configured component exists, has matching
embedded plugin metadata, implements a supported role, publishes a schema that
accepts its settings, and has sufficient consent for admission. Use
`--config /path/to/sage.toml` with either `grants` or `plugins` to select another
configuration.

### Typed plugin settings

Configuration has a deliberate ownership boundary. SAGE owns `component`; a
plugin owns its `[plugins.<id>.settings]` table. The framework-generated JSON
Schema uses that settings object as its root and does not describe the
surrounding plugin entry.

At startup, the host converts the complete settings table to one JSON object.
Strings, numbers, booleans, arrays, and nested tables retain their corresponding
JSON types. `api-key-env` is host-managed: SAGE removes it from the delivered
object and, when `api-key` is not already present, reads the selected environment
variable and inserts its value as `api-key`. Lockgate validates that object at
prepare time against the schema derived from the plugin's `type Settings`. The
plugin then reads the validated typed value through `Self::settings()`.

Rust plugins derive both deserialization and schema behavior from one strict
settings type, as the built-in [OpenAI-compatible
provider](plugins/openai-compatible/src/lib.rs) and [Kagi
tools](plugins/kagi/src/lib.rs) do:

```rust
#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct Settings {
    base_url: String,
    egress_origin: String,
    model: String,
    #[serde(default)]
    api_key: Option<String>,
}
```

Lockgate fetches the framework schema and validates resolved settings before
admitting the plugin, loading tool definitions, or using a provider. Invalid
settings are reported as plugin admission errors. Closed v2 settings schemas
carry `unevaluatedProperties: false`, so properties introduced by composition
cannot bypass the plugin's declared settings contract:

```text
failed to load plugin `openai`: plugin settings do not satisfy the schema
```

`cargo run -- plugins check` exercises this preparation and admission path. The
old `settings-host` and `outbound-http` application interfaces are gone: v2
injects typed settings through the framework contract and links HTTP only from
the plugin's declared `net::EGRESS` grants. See [the WIT
contract](crates/sage-plugin/wit/sage.wit) for the SAGE-owned role interfaces;
Lockgate adds its configuration interfaces automatically.

## Development

Run the project with the system Cargo:

```console
cargo run -- --help
```

With direnv installed, `direnv allow` enters the same environment automatically.
Entering the development shell also installs the configured Git hooks. Formatting
and Clippy run before commits, while the test suite runs before pushes. Run the
commit hooks manually with:

```console
pre-commit run --all-files
```
