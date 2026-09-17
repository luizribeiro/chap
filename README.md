# CHAP

CHAP (Consent-Honoring Agent Platform) is a coding agent with an embeddable core, user-facing frontends, and capability-scoped WebAssembly plugins provided by Lockgate.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/luizribeiro/chap/main/install.sh | sh
```

## Supported platforms

See [the installation guide](docs/install.md#supported-platforms) for platform requirements.

## Installer options

See [the installer options](docs/install.md#installer-options) for version and path overrides.

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

See [download verification](docs/install.md#verify-a-download) for checksum commands.

## Uninstall

See [uninstall instructions](docs/install.md#uninstall) for removing CHAP and its state.

## Workspace

See [the workspace overview](docs/development.md#workspace) for the repository layout and source-run command.

## Plugins

See [the plugin reference](docs/plugins.md) for configuration, capabilities, budgets, grants, and settings.

## Nix

See [the Nix guide](docs/nix.md) for declarative instances and project templates.

## Development

See [the development guide](docs/development.md#running-locally) for the local workflow and checks.
