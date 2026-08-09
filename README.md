# SAGE

SAGE (Sandboxed Agent with Guarded Extensions) is a coding agent built around
an iocraft terminal interface and capability-scoped WebAssembly plugins provided
by Lockgate.

## Plugins

Plugins and their configuration live in `sage.toml`. List the configured
plugins with:

```console
cargo run -- plugins list
```

Use `--config /path/to/sage.toml` to read a different file.

Each plugin is keyed by its stable id and maps directly to its component and
settings:

```toml
[plugins.openai]
component = "./plugins/openai-compatible.wasm"

[plugins.openai.settings]
model = "example-model"
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
