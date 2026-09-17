# CHAP

CHAP (Consent-Honoring Agent Platform) is a coding agent with an embeddable core, user-facing frontends, and capability-scoped WebAssembly plugins provided by Lockgate.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/luizribeiro/chap/main/install.sh | sh
```

## Supported platforms

- Apple Silicon macOS (`aarch64-apple-darwin`)
- Linux on x86-64 (`x86_64-unknown-linux-gnu`) or ARM64 (`aarch64-unknown-linux-gnu`), with `/dev/kvm` available on bare metal or through nested virtualization and the `libcap-ng` shared library installed (`libcap-ng0` on Debian/Ubuntu or `libcap-ng` on Fedora)

The sandbox runtime keeps its sockets under `${XDG_STATE_HOME:-$HOME/.local/state}/chap/msb`, and unix socket paths are capped at 104 bytes on macOS, so that directory must be at most 51 characters long. If your home directory is long, export `MSB_HOME` pointing at a shorter directory before running `chap`; the wrapper refuses to start otherwise and says so.

## Installer options

The installer accepts these optional environment variables:

- `CHAP_VERSION`: release version, with or without the leading `v`; defaults to the latest release
- `CHAP_INSTALL_DIR`: installation root; defaults to `$HOME/.local/share/chap`
- `CHAP_BIN_DIR`: directory for the `chap` symlink; defaults to `$HOME/.local/bin`
- `CHAP_TARBALL`: local release tarball to install instead of downloading one; its `.sha256` sibling is checked when present
- `CHAP_TARGET`: target override, normally detected from the operating system and architecture

For example, to install a specific version:

```sh
curl -fsSL https://raw.githubusercontent.com/luizribeiro/chap/main/install.sh | CHAP_VERSION=1.2.3 sh
```

## First run

Edit `~/.config/chap/chap.json` (or `$XDG_CONFIG_HOME/chap/chap.json`) and set `base_url`, `model`, and `api_key_env` for your OpenAI-compatible provider, then export the API key in the variable named by `api_key_env` (`OPENAI_API_KEY` by default).

CHAP only loads plugins you have approved. The first launch lists the plugins that still need approval and the exact commands to run; for the default configuration they are:

```sh
chap grants review openai && chap grants approve openai
chap grants review workspace && chap grants approve workspace
chap grants review persona && chap grants approve persona
```

Then start it from the directory you want the agent to work in; that directory is mounted read-write at `/mnt/workspace` inside the sandbox VM:

```sh
chap
```

## Verify a download

Download the release tarball and its matching `.sha256` file from this repository's [Releases page](https://github.com/luizribeiro/chap/releases), then run one of:

```sh
sha256sum -c chap-X.Y.Z-<target>.tar.gz.sha256
shasum -a 256 -c chap-X.Y.Z-<target>.tar.gz.sha256
```

## Uninstall

With the default paths, remove the installation and symlink with:

```sh
rm -rf "$HOME/.local/share/chap"
rm -f "$HOME/.local/bin/chap"
```

Optionally remove the configuration too:

```sh
rm -rf "${XDG_CONFIG_HOME:-$HOME/.config}/chap"
```

Optionally remove the state directory too; it holds plugin approvals, cached VM images and sandboxes:

```sh
rm -rf "${XDG_STATE_HOME:-$HOME/.local/state}/chap"
```

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

See [the plugin reference](docs/plugins.md) for configuration, capabilities, budgets, grants, and settings.

## Declarative instances

Nix flakes can compose a checked CHAP instance with `mkChap`. The result contains
`etc/chap.json` and a wrapped `bin/chap` that uses that configuration by default.
Plugin `settings` merge shallowly over the package's default settings, and the
top-level `settings` sections are emitted as given: a nested attribute set
replaces the whole section rather than merging into it. Feature-gated plugins
also need the corresponding host package; for example, set
`package = chap.packages.${system}.chap-vm` for a VM-enabled instance.
This downstream flake combines a bundled provider with a third-party component
built through `buildChapPlugin`:

```nix
{
  inputs = {
    chap.url = "github:luizribeiro/chap";
    nixpkgs.follows = "chap/nixpkgs";
  };

  outputs =
    { chap, nixpkgs, ... }:
    let
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-darwin"
        "x86_64-linux"
      ];
    in
    {
      packages = nixpkgs.lib.genAttrs systems (
        system:
        let
          weather = chap.lib.${system}.buildChapPlugin {
            pname = "chap-weather";
            version = "0.1.0";
            src = ./plugin;
            defaultSettings.units = "metric";
          };
        in
        {
          default = chap.lib.${system}.mkChap {
            name = "work";
            plugins = {
              openai = {
                plugin = chap.packages.${system}.plugin-openai-compatible;
                settings = {
                  base_url = "http://127.0.0.1:8080/v1";
                  model = "local-model";
                  api_key_env = "OPENAI_API_KEY";
                };
              };
              weather = {
                plugin = weather;
                settings.location = "New York";
              };
            };
          };
        }
      );
    };
}
```

Consent approvals live per user below the XDG state directory, separated by
instance name. Settings are world-readable in the Nix store, so they must never
contain secrets; use environment indirection such as `api_key_env` instead.

### Templates

`nix flake init -t github:luizribeiro/chap#instance` creates a minimal
declarative instance flake. Plugin authors can use
`nix flake init -t github:luizribeiro/chap#plugin` for a working tools-plugin
skeleton; generate its `Cargo.lock` before the first Nix build.

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
