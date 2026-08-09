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

Check that every configured component exists, has matching embedded plugin
metadata, and implements SAGE's provider interface with:

```console
cargo run -- plugins check
```

The repository includes a placeholder OpenAI-compatible provider. From the Nix
development shell, build it from its source directory before checking configured
plugins:

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
