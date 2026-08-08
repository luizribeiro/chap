# SAGE

SAGE (Sandboxed Agent with Guarded Extensions) is a coding agent built around
an iocraft terminal interface and capability-scoped WebAssembly plugins provided
by Lockgate.

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
