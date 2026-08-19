# SAGE

SAGE (Sandboxed Agent with Guarded Extensions) is a coding agent with a headless
core, user-facing frontends, and capability-scoped WebAssembly plugins provided
by Lockgate.

## Workspace

- `crates/sage-core` owns configuration, plugin loading, and the headless agent
  runtime.
- `crates/sage-cli` builds the `sage` executable and owns command-line and
  terminal interaction.
- `plugins` contains independently compiled WebAssembly components.
- `wit` contains the application-owned contracts shared by the core and plugins.

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

Check that every configured component exists, has matching embedded plugin
metadata, implements a supported role, and publishes a schema that accepts its
configured settings with:

```console
cargo run -- plugins check
```

The repository includes an OpenAI-compatible Chat Completions provider that uses
WASI HTTP. Build it with the system Cargo before checking configured plugins:

```console
cargo build -p sage-openai-compatible --release --target wasm32-wasip2
cargo run -- plugins check
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

The OpenAI-compatible plugin declares network egress through Lockgate and
resolves its exact origin from `egress-origin`. The scheme and effective port
are part of the origin. Kagi declares the literal origin `https://kagi.com` and
therefore needs no configurable origin.

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
settings are reported as plugin admission errors:

```text
failed to load plugin `openai`: plugin settings do not satisfy the schema
```

`cargo run -- plugins check` exercises this preparation and admission path. See
[the WIT contract](wit/sage.wit) for the SAGE-owned role interfaces; Lockgate
adds its settings and schema interfaces automatically.

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
