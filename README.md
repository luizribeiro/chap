# CHAP

CHAP (Consent-Honoring Agent Platform) is a coding agent with an embeddable core, user-facing frontends, and capability-scoped WebAssembly plugins provided by Lockgate.

## Workspace

- `crates/chap-core` owns configuration, plugin loading, and the embeddable agent
  runtime.
- `crates/chap-cli` builds the `chap` executable and owns command-line and
  terminal interaction.
- `crates/chap-plugin` is the thin plugin-author facade over Lockgate and owns
  the shared `chap:agent` WIT package.
- `plugins` contains independently compiled WebAssembly components.

The CLI is the default workspace member, so root-level `cargo run` commands keep
working while other frontends can depend directly on `chap-core`.

Running CHAP without a subcommand opens its terminal interface using the
configured `openai` provider:

```console
cargo run
```

## Plugins

Plugins and their configuration live in `chap.json`. List the configured
plugins with:

```console
cargo run -- plugins list
```

Use `--config /path/to/chap.json` to read a different file.

The repository includes an OpenAI-compatible Chat Completions provider and Kagi
web tools. Build their configured release components with the system Cargo:

```console
cargo build -p chap-openai-compatible -p chap-kagi --release --target wasm32-wasip2
```

Each plugin is keyed by an operator-assigned instance id and maps directly to
its component and settings:

```json
{
  "plugins": {
    "openai": {
      "component": "./target/wasm32-wasip2/release/chap_openai_compatible.wasm",
      "settings": {
        "base-url": "http://127.0.0.1:8080/v1",
        "model": "example-model",
        "api-key-env": "OPENAI_API_KEY"
      }
    }
  }
}
```

`api-key-env` is optional and names the environment variable holding the API
key. The plugin declares an `env.read` grant for it, so the access appears in
`chap grants review`; the value itself never appears in `chap.json` and is never
read by the host. If it is omitted, the plugin requests no environment grant
and sends no authorization header.

### Tool execution

Tool calls run in parallel by default. Configure the host-wide mode and
concurrency limit in `chap.json`:

```json
{
  "tools": {
    "execution": "parallel",
    "max-concurrency": 8
  }
}
```

The values shown are the defaults. `"execution": "sequential"` runs every batch
one call at a time. In parallel mode, any call whose tool resolves to sequential
makes the whole batch sequential; otherwise `max-concurrency` limits the number
of calls in flight. The limit must be at least 1 and has no fixed upper bound.

A plugin's `tools` section can tighten all tools loaded from that plugin:

```json
{
  "plugins": {
    "stateful-tools": {
      "component": "./stateful-tools.wasm",
      "tools": {
        "execution": "sequential"
      }
    }
  }
}
```

Plugin overrides are tighten-only: `sequential` can restrict a plugin, while
`parallel` cannot loosen a tool's own sequential declaration. The plugin author
knows whether a tool has shared internal state; an operator does not, so operator
configuration may narrow scheduling but never widen it.

Raise `max-concurrency` deliberately. Each in-flight plugin call keeps a live
wasmtime `Store` with its own memory allowance under `RuntimeLimits::default()`,
so higher limits have a real memory cost.

Each built-in plugin carries its own Cargo configuration and defaults to the
`wasm32-wasip2` target. The development shell provides Wasmtime as Cargo's test
runner, so `cargo test`, `cargo check`, and `cargo clippy` work directly from a
plugin directory without target flags.

From the workspace root, run the complete native and WASI test suites with:

```console
cargo test-all
```

The OpenAI-compatible plugin declares network egress scoped from `base-url`,
and Lockgate resolves that URL's origin as the granted scope. The scheme and
effective port are part of the origin. Kagi declares the literal origin
`https://kagi.com` and therefore needs no configurable origin.

### Token usage

Provider plugins may report input and output token usage with each completion,
plus optional cached-input and reasoning counters. Cached input is a subset of
input, and reasoning is a subset of output, so neither is added to the totals.
An absent subset counter means the server did not report it, distinct from zero;
this is normal for local servers such as vLLM and llama.cpp.

CHAP accumulates reported usage across every provider step. The footer forecasts
the next prompt from the latest step's input and output, with session-wide input
and output totals and nonzero subset totals. Providers omitting optional `usage`
remain supported: CHAP invents no counters and the footer stays hidden.

Cache writes are separately billed rather than part of input. The bundled
OpenAI-compatible plugin reads vLLM's `prompt_tokens_details.created_cache_tokens`;
other servers may leave it absent, while Anthropic-style providers can report it.

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
that instance's approval. CHAP stores approvals in `consent.json` beside the
selected `chap.json`; concrete scopes remain in `chap.json`. A permission
expansion, such as changing `base-url` to point at a different origin, blocks
admission until the new manifest is reviewed and approved. Narrowing or
removing authority is reported as non-blocking drift.

`plugins check` verifies that every configured component exists, has matching
embedded plugin metadata, implements a supported role, publishes a schema that
accepts its settings, and has sufficient consent for admission. Use
`--config /path/to/chap.json` with either `grants` or `plugins` to select another
configuration.

### Typed plugin settings

Configuration has a deliberate ownership boundary. CHAP owns `component`; a
plugin owns its `plugins.<id>.settings` object. The framework-generated JSON
Schema uses that settings object as its root and does not describe the
surrounding plugin entry.

At startup, the host passes the complete settings object through as JSON.
Strings, numbers, booleans, arrays, and nested objects retain their JSON types.
Secrets stay out of that object entirely: a setting such as
`api-key-env` carries only the *name* of an environment variable, the plugin
declares an `env.read` need scoped from it, and Lockgate populates the
component's environment with just the granted variables at admission. The plugin
reads the key with `std::env::var`; the host never touches the value. That need
is optional: an optional need whose setting is absent drops out of the resolved
manifest, so a configuration without `api-key-env` is admitted with no
environment grant and the provider sends no authorization header. Lockgate
validates the settings object at prepare time against the schema derived from
the plugin's `type Settings`. The plugin then reads the validated typed value
through `Self::settings()`.

Rust plugins depend on `serde` and `schemars` directly and derive both
deserialization and schema behavior on one strict settings type, as the built-in
[OpenAI-compatible provider](plugins/openai-compatible/src/lib.rs) and [Kagi
tools](plugins/kagi/src/lib.rs) do:

```rust
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Settings {
    base_url: String,
    model: String,
    api_key_env: Option<String>,
}
```

An `Option<T>` field is optional in the derived schema, so a settings object that
omits it still validates. Lockgate fetches the framework schema and validates
resolved settings before admitting the plugin, loading tool definitions, or
using a provider. Invalid settings are reported as plugin admission errors.
Closed v2 settings schemas
carry `unevaluatedProperties: false`, so properties introduced by composition
cannot bypass the plugin's declared settings contract:

```text
failed to load plugin `openai`: plugin settings do not satisfy the schema
```

`cargo run -- plugins check` exercises this preparation and admission path. The
old `settings-host` and `outbound-http` application interfaces are gone: v2
injects typed settings through the framework contract and links HTTP only from
the plugin's declared `net::EGRESS` grants. See [the WIT
contracts](crates/chap-plugin/wit) for the CHAP-owned role interfaces;
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
