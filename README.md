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
WASI HTTP. From the Nix development shell, build it from its
source directory before checking configured plugins:

```console
cd plugins/openai-compatible
cargo build --release -Z build-std=std,panic_abort --target wasm32-wasip2
cd ../..
cargo run -- plugins check
```

Each plugin is keyed by its stable id and maps directly to its component and
settings. The key must match the stable plugin id embedded in the component:

```toml
[plugins.openai]
component = "./target/wasm32-wasip2/release/sage_openai_compatible.wasm"
outbound-http = ["http://127.0.0.1:8080"]

[plugins.openai.settings]
base-url = "http://127.0.0.1:8080/v1"
model = "example-model"
# Optional: the host reads this environment variable without storing its value
# in sage.toml.
api-key-env = "OPENAI_API_KEY"
```

`outbound-http` is an allowlist of exact origins. The scheme and effective port are part of the
policy: `https://api.example.com` allows port 443 only, while a local service on another port must
be written explicitly, such as `http://127.0.0.1:8080`. Omitting the field denies outbound HTTP.

### Typed plugin settings

Configuration has a deliberate ownership boundary. SAGE parses and validates
fields it understands, including `component` and `outbound-http`. A plugin owns
only its `[plugins.<id>.settings]` table, and the JSON Schema it publishes uses
that settings object as its root; it does not describe the surrounding plugin
entry. If a field is eventually shared by every plugin in a role, it belongs in
SAGE-owned typed configuration and/or that role's WIT interface rather than in
each plugin's private schema.

At startup, the host converts the complete settings table to one JSON object.
Strings, numbers, booleans, arrays, and nested tables retain their corresponding
JSON types. `api-key-env` is host-managed: SAGE removes it from the delivered
object and, when `api-key` is not already present, reads the selected environment
variable and inserts its value as `api-key`. The plugin obtains the resulting
object from `settings.get-json` and should deserialize it once into a strict,
typed settings structure.

Provider and tool plugins must export `configuration.settings-schema`. It
returns a JSON Schema Draft 2020-12 document for the same resolved JSON object
the plugin receives. Rust plugins can derive the schema from the type they
deserialize, as the built-in
[OpenAI-compatible provider](plugins/openai-compatible/src/lib.rs) and
[Kagi tools](plugins/kagi/src/lib.rs) do:

```rust
#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct Settings {
    base_url: String,
    model: String,
    #[serde(default)]
    api_key: Option<String>,
}

let schema = schemars::generate::SchemaSettings::draft2020_12()
    .into_generator()
    .into_root_schema_for::<Settings>();
let schema_json = serde_json::to_string(&schema)?;
```

The export should be pure and deterministic: it should not read settings,
environment variables, clocks, or the network. SAGE starts the sandbox, calls
the schema export once for each configured plugin, and validates its resolved
settings before loading tool definitions or using a provider. Invalid settings
are user configuration errors reported with the configuration filename and a
logical TOML path, for example:

```text
sage.toml: plugins.openai.settings.model: required setting is missing
```

A missing configuration export or a malformed schema is instead a plugin
contract error. `cargo run -- plugins check` exercises this startup path, so it
validates settings and schemas in addition to component identity and supported
roles.

The schema is retrieved through the WIT method at runtime; it is not embedded in
or read from component metadata. See [the WIT contract](wit/sage.wit) for the
authoritative interface definitions.

## Development

Enter the Nix development shell and run the project:

```console
nix develop
cargo run -- --help
```

With direnv installed, `direnv allow` enters the same environment automatically.
Entering the development shell also installs the configured Git hooks. Formatting
and Clippy run before commits, while the test suite runs before pushes. Run the
commit hooks manually with:

```console
pre-commit run --all-files
```
