# CHAP

CHAP (Consent-Honoring Agent Platform) is a coding agent that only does what you have consented to. Its tools run inside a sandboxed virtual machine, and its capabilities come from WebAssembly plugins managed by Lockgate.

## Install

### macOS (Apple Silicon)

Install with Homebrew:

```sh
brew install luizribeiro/chap/chap
```

### Linux (x86_64 and ARM64)

Requires `/dev/kvm` and the `libcap-ng` library:

```sh
curl -fsSL https://raw.githubusercontent.com/luizribeiro/chap/main/install.sh | sh
```

### Nix

Run CHAP once, or initialize a declarative instance; see the [Nix guide](docs/nix.md):

```sh
nix run github:luizribeiro/chap#chap-vm -- --config chap.json
nix flake init -t github:luizribeiro/chap#instance
```

See the [installation guide](docs/install.md) for options, checksums, uninstalling, and the state directory.

## First run

Create `~/.config/chap/chap.json`. The macOS and Linux installers render one from `share/chap/chap.json.in`; Nix users declare it as part of their instance. Set `base_url`, `model`, and `api_key_env` for your OpenAI-compatible provider, then export the named API key:

```sh
export OPENAI_API_KEY="..."
```

CHAP refuses to load a plugin until you approve its grants:

```sh
chap grants review <plugin>
chap grants approve <plugin>
```

Run CHAP from the project directory. That directory is mounted read-write at `/mnt/workspace` inside the sandbox:

```sh
chap
```

## Learn more

- [Plugin reference](docs/plugins.md) — configuration, capabilities, budgets, and grants.
- [Nix guide](docs/nix.md) — declarative instances and templates.
- [Installation guide](docs/install.md) — platform details and lifecycle tasks.
- [Development guide](docs/development.md) — repository layout and local workflow.

## Status

CHAP is early software, so expect rough edges. No license has been chosen yet; all rights are reserved for now.
