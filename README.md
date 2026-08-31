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

CHAP resolves its configuration in this order:

1. The path passed to `--config`.
2. The path in `CHAP_CONFIG`.
3. `./chap.json`, when that file exists.
4. `$XDG_CONFIG_HOME/chap/chap.json`, or `~/.config/chap/chap.json` when
   `XDG_CONFIG_HOME` is unset or empty.

An existing local `./chap.json` is authoritative. If it cannot be read or
parsed, CHAP reports that error instead of falling through to the personal
configuration.

The repository includes an OpenAI-compatible Chat Completions provider, Kagi
web tools, and a persona context contributor. The exec plugin exposes an
argv-style process tool mediated by command-prefix grants.

Build the configured release components with the system Cargo:

```console
cargo build -p chap-openai-compatible -p chap-exec-plugin -p chap-kagi -p chap-persona --release --target wasm32-wasip2
```

Each plugin is keyed by an operator-assigned instance id and maps directly to
its component and settings:

```json
{
  "name": "work",
  "plugins": {
    "openai": {
      "component": "./target/wasm32-wasip2/release/chap_openai_compatible.wasm",
      "settings": {
        "base_url": "http://127.0.0.1:8080/v1",
        "model": "example-model",
        "api_key_env": "OPENAI_API_KEY",
        "timeouts": {
          "connect_seconds": 10,
          "first_byte_seconds": 60,
          "between_bytes_seconds": 30
        }
      }
    },
    "persona": {
      "component": "./target/wasm32-wasip2/release/chap_persona.wasm",
      "context": {
        "channel": "system"
      },
      "settings": {
        "persona": "You are a wizard. Answer in riddles.",
        "priority": 0
      }
    }
  }
}
```

The optional top-level `name` identifies this CHAP instance. It may contain
letters, digits, `.`, `_`, and `-`, except the reserved values `.` and `..`.

`api_key_env` is optional and names the environment variable holding the API
key. The plugin declares an `env.read` grant for it, so the access appears in
`chap grants review`; the value itself never appears in `chap.json` and is never
read by the host. If it is omitted, the plugin requests no environment grant
and sends no authorization header.

The optional `timeouts` object configures the HTTP transport in whole, nonzero
seconds. Each timeout is independently optional: `connect_seconds` limits
connection establishment, `first_byte_seconds` limits the wait for the first
response byte, and `between_bytes_seconds` limits stalls between later response
body chunks. All three are unset when omitted. They provide specific transport
errors within the provider call's separate, outer wall-clock deadline.

### Configuration layers

Within the selected `chap.json`, settings have three configuration layers:

| Layer | Key | Reader and scope |
| --- | --- | --- |
| Plugin settings | `plugins.<id>.settings` | The guest, for its own internals |
| Role settings | `plugins.<id>.tools` or `.context` | CHAP, per plugin role |
| Agent settings | `agent` | CHAP, for the agent regardless of loaded plugins |

The names use one noun, "settings", qualified by the scope it applies to.

Plugin settings are opaque to CHAP: the host passes the JSON object through to
the guest unchanged and never interprets it. [Typed plugin
settings](#typed-plugin-settings) covers the plugin-published schema contract
and secret handling.

No role name is ever a top-level key. This is why agent-wide tool-execution
settings live at `agent.tool_execution` rather than in a top-level `tools`
section.

### Exec capability

The optional, host-owned `agent.exec` section configures command resolution,
environment passthrough, and the hard timeout ceiling:

```json
{
  "agent": {
    "exec": {
      "path": ["/usr/local/bin", "/usr/bin", "/bin"],
      "env_passthrough": ["CARGO_HOME", "RUSTUP_HOME"],
      "timeout_ceiling_ms": 120000
    }
  }
}
```

`path` is a list of directories used to resolve bare program names; when it is
omitted, CHAP snapshots its startup `PATH`. `env_passthrough` names additional
host variables to copy, and `timeout_ceiling_ms` caps every plugin-requested
deadline and defaults to 120 seconds.

Spawned processes receive a constructed environment, never CHAP's inherited
environment. CHAP supplies the pinned `PATH`, copies `HOME`, `TERM`, `LANG`, and
`TMPDIR` when present, and adds only variables named in `env_passthrough`.
Provider API keys are therefore not visible to spawned processes unless an
operator explicitly names them for passthrough. Commands run from the directory
where CHAP was invoked.

The entire stack is behind the Cargo `exec` feature, which is off by default:

```console
cargo build -p chap-cli --features exec
```

A build without the feature refuses an `agent.exec` section at startup and
refuses any plugin needing `exec.run` at admission.

### Tool execution

Tool calls run in parallel by default. Configure the host-wide mode and
concurrency limit in `chap.json`:

```json
{
  "agent": {
    "tool_execution": {
      "mode": "parallel",
      "max_concurrency": 8
    }
  }
}
```

The values shown are the defaults. Setting `agent.tool_execution.mode` to
`"sequential"` runs every batch one call at a time. In parallel mode, any call
whose tool resolves to sequential makes the whole batch sequential; otherwise
`agent.tool_execution.max_concurrency` limits the number of calls in flight.
The limit must be at least 1 and has no fixed upper bound.

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

Scheduling is tighten-only at both levels: `agent.tool_execution.mode` can force
every batch to run sequentially, while `plugins.<id>.tools.execution` can force
one plugin's tools to run sequentially. Neither can loosen a tool's own
sequential declaration. The plugin author knows whether a tool has shared
internal state; an operator does not, so operator configuration may narrow
scheduling but never widen it.

Raise `agent.tool_execution.max_concurrency` deliberately. Each in-flight plugin
call keeps a live wasmtime `Store` with its own memory allowance under
`RuntimeLimits::default()`, so higher limits have a real memory cost.

Each built-in plugin carries its own Cargo configuration and defaults to the
`wasm32-wasip2` target. The development shell provides Wasmtime as Cargo's test
runner, so `cargo test`, `cargo check`, and `cargo clippy` work directly from a
plugin directory without target flags.

From the workspace root, run the complete native and WASI test suites with:

```console
cargo test-all
```

The OpenAI-compatible plugin declares network egress scoped from `base_url`,
and Lockgate resolves that URL's origin as the granted scope. The scheme and
effective port are part of the origin. Kagi declares the literal origin
`https://kagi.com` and therefore needs no configurable origin. Persona exports
only the context role and requests no capabilities; it contributes its
configured text at session creation without storing it in session history.

Context plugins use the `context` channel by default, contributing one leading
user message. Set a plugin's `context.channel` to `system` when that plugin
speaks as the operator; system context is placed first, followed by contributed
context and then conversation history. This distinction is a soft model prior,
not a security boundary—the grant system controls what plugins and the model can
actually do.

CHAP does not wrap, attribute, or escape contributed content. A plugin relaying
third-party bytes should provide its own wrapper and escaping.

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
cargo run -- grants approve persona
cargo run -- plugins check
```

With an exec-enabled build, configure the exec plugin's command scopes in its
settings:

```json
{
  "plugins": {
    "exec": {
      "component": "./target/wasm32-wasip2/release/chap_exec_plugin.wasm",
      "settings": {
        "allowed_commands": ["cargo", "git commit", "rg"]
      }
    }
  }
}
```

Command scopes are argv prefixes matched by exact tokens, not string prefixes.
A bare program name is the one-token case. `git commit` allows
`git commit -m ...`, but not `git push`; `git -c x=y commit` does not match and
is denied. Fail-to-match is fail-safe. The resolved prefixes are digest-bound,
appear in `chap grants review`, and changing `allowed_commands` is scope drift
that requires review and re-approval.

Prefix scopes bound entry points, not effects: anything after a matched prefix
is unconstrained, and allowing `sh`, interpreters, or build tools is arbitrary
code execution by design. The constructed environment and the future sandbox
stage are the compensating layers.

`grants review` also accepts one instance id. `grants deny <instance-id>` removes
that instance's approval. CHAP stores approvals below `$XDG_STATE_HOME/chap`, or
`$HOME/.local/state/chap` when `XDG_STATE_HOME` is unset or empty. Named
configurations use `named/<name>/consent.json`; unnamed configurations use
`by-path/<sha256-of-absolute-config-path>/consent.json`. Concrete scopes remain
in `chap.json`. A permission expansion, such as changing `base_url` to point at
a different origin, blocks admission until the new manifest is reviewed and
approved. Narrowing or removing authority is reported as non-blocking drift.
Approval also binds the component's exported interfaces: a plugin that starts
exporting a role it was not approved for blocks admission the same way, while
dropping a role or changing only an interface version is non-blocking.

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
`api_key_env` carries only the *name* of an environment variable, the plugin
declares an `env.read` need scoped from it, and Lockgate populates the
component's environment with just the granted variables at admission. The plugin
reads the key with `std::env::var`; the host never touches the value. That need
is optional: an optional need whose setting is absent drops out of the resolved
manifest, so a configuration without `api_key_env` is admitted with no
environment grant and the provider sends no authorization header. Lockgate
validates the settings object at prepare time against the schema derived from
the plugin's `type Settings`. The plugin then reads the validated typed value
through `Self::settings()`.

Rust plugins depend on `serde` and `schemars` directly and derive both
deserialization and schema behavior on one strict settings type, as the built-in
[OpenAI-compatible provider](plugins/openai-compatible/src/lib.rs) and [Kagi
tools](plugins/kagi/src/lib.rs), along with the [Persona context
plugin](plugins/persona/src/lib.rs), do:

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
contracts](crates/chap-wit/wit/) for the CHAP-owned role interfaces;
Lockgate adds its configuration interfaces automatically.

## Declarative instances

Nix flakes can compose a checked CHAP instance with `mkChap`. The result contains
`etc/chap.json` and a wrapped `bin/chap` that uses that configuration by default.
Plugin `settings` merge shallowly over the package's default settings, and the
top-level `settings` sections are emitted as given: a nested attribute set
replaces the whole section rather than merging into it.
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
