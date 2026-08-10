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
metadata, and implements SAGE's provider interface with:

```console
cargo run -- plugins check
```

The repository includes an OpenAI-compatible Chat Completions provider that uses
a host-provided HTTP client. From the Nix development shell, build it from its
source directory before checking configured plugins:

```console
cd plugins/openai-compatible
cargo build --release -Z build-std=std,panic_abort --target wasm32-wasip3
cd ../..
cargo run -- plugins check
```

Each plugin is keyed by its stable id and maps directly to its component and
settings. The key must match the stable plugin id embedded in the component:

```toml
[plugins.openai]
component = "./target/wasm32-wasip3/release/sage_openai_compatible.wasm"

[plugins.openai.settings]
base-url = "http://127.0.0.1:8080/v1"
model = "example-model"
# Optional: the host reads this environment variable without storing its value
# in sage.toml.
api-key-env = "OPENAI_API_KEY"
```

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
