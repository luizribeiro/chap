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
web tools, a persona context contributor, and exec, state, and VM sandbox tools.
The exec plugin exposes an argv-style host process tool mediated by
command-prefix grants; the sandbox plugin runs argv commands in a microVM.

Build the configured release components with the system Cargo:

```console
cargo build -p chap-openai-compatible -p chap-exec-plugin -p chap-state-plugin -p chap-vm-plugin -p chap-kagi -p chap-persona --release --target wasm32-wasip2
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

The optional, host-owned `agent.exec` section configures command resolution and
the hard timeout ceiling:

```json
{
  "agent": {
    "exec": {
      "path": ["/usr/local/bin", "/usr/bin", "/bin"],
      "timeout_ceiling_ms": 120000,
      "max_concurrent_processes": 4
    }
  }
}
```

`path` is a list of directories used to resolve bare program names; empty
entries are ignored, and relative entries are made absolute against CHAP's
startup directory. When `path` is omitted, CHAP applies those rules while
snapshotting its startup `PATH`. `timeout_ceiling_ms` caps every
plugin-requested deadline and defaults to 120 seconds. The effective timeout is
the minimum of the plugin's request, this ceiling, and the tools call deadline.
The tools deadline defaults to 30 seconds, so the 120-second ceiling is not
reached unless `agent.budgets.tools.deadline_ms` is also raised.
`max_concurrent_processes` limits simultaneous exec spawns from each plugin,
must be at least 1, and defaults to 4.

Spawned processes receive a constructed environment, never CHAP's inherited
environment: CHAP supplies the pinned `PATH` and copies `HOME`, `TERM`, `LANG`,
and `TMPDIR` when present, and nothing else. Provider API keys are therefore not
visible to spawned processes. Commands run from the directory where CHAP was
invoked. `HOME` and `TMPDIR` are the real host values, so `~/.gitconfig`,
`~/.cargo/config.toml`, repo-local `.cargo/config.toml`, and `.git/hooks` all
apply. The startup `PATH` snapshot may include user-writable directories.

There is deliberately no way to forward host variables or let a plugin set its
own. Environment variables are a program-independent execution channel: many
tools read them as configuration that redirects what actually runs — `LD_PRELOAD`
ahead of a process's `main`, git's `GIT_SSH_COMMAND` and `GIT_CONFIG_*`,
`NODE_OPTIONS`, and more. Because commands run directly on the host, honoring a
plugin-supplied environment would turn even a narrowly scoped grant such as
`git status` into arbitrary code execution, sidestepping the argv-prefix scope
that consent is built on. A fixed environment keeps narrowly scoped host
execution from gaining this ambient channel. Commands that need a
plugin-controlled environment or stronger isolation belong behind the VM
capability, where the sandbox boundary — not a filter over variable names —
contains them.

The entire stack is behind the Cargo `exec` feature, which is off by default:

```console
cargo build -p chap-cli --features exec
```

A build without the feature refuses an `agent.exec` section at startup and
refuses any plugin needing `exec.run` at admission.

### VM capability

The optional, host-owned `agent.vm` section sets global microVM limits and the
OCI registries that plugins may use:

```json
{
  "agent": {
    "vm": {
      "registries": [],
      "limits": { "max_vms_per_plugin": 8 },
      "instance": {
        "cpus": 1,
        "memory_mb": 512,
        "max_lifetime_ms": 3600000,
        "idle_timeout_ms": 300000
      },
      "calls": {
        "create_timeout_ms": 600000,
        "destroy_timeout_ms": 60000,
        "exec_timeout_ceiling_ms": 120000,
        "exec_max_output_bytes": 65536,
        "read_file_max_bytes": 16777216
      }
    }
  }
}
```

The values shown are the defaults. `limits.max_vms_per_plugin` bounds each
plugin instance separately. `instance` defines the CPU count, memory, maximum
lifetime, and idle timeout that every new VM receives. `calls` bounds single
host calls: `create_timeout_ms` covers each existence lookup and creation call,
including the image pull and boot; `destroy_timeout_ms` covers destruction plus
the startup and shutdown reaps; `exec_timeout_ceiling_ms` is both the default
and the maximum for a command's `timeout-ms`; `exec_max_output_bytes` caps
stdout and stderr together; and `read_file_max_bytes` caps one file read.

`registries` is empty by default, so even a fully qualified image is denied
until the operator lists its registry. In particular, add `docker.io` to pull
from Docker Hub. Image references must include a registry and repository plus
either an explicit tag, such as `docker.io/library/alpine:3.20`, or a
`sha256` digest. Unqualified names and local paths are rejected, repository
names are lowercase, and the registry must exactly match an allowed entry.

VM authority is split into four permissions:

- `vm.create` is the flag that permits creation and get-or-create.
- `vm.mount` scopes expose absolute host paths as `ro:/path` or `rw:/path`.
  `rw:` permits either access mode within that path, while `ro:` requires the
  mount to be read-only. Colons elsewhere are legal path characters; only the
  leading `ro:` or `rw:` is the mode marker. Grants are matched against the
  canonical host path: on macOS, for example, grant `rw:/private/tmp/project`,
  not `rw:/tmp/project`.
- `vm.egress` scopes contain an IP address or strict CIDR plus a port:
  `addr:port`, `addr/prefix:port`, `[v6]:port`, or `[v6]/prefix:port`. The CIDR
  address must be the network address, and `*` in the port position permits any
  port.
- `vm.manage` with the `created-by-caller` scope permits get, exec, file reads
  and writes, and destroy for VMs created by that caller.

VM identities include the plugin instance and session. Each plugin can see and
manage only its own namespace, even when another plugin uses the same logical VM
name. The backend labels each sandbox with an installation and process epoch,
reaps older epochs for that installation at startup, and destroys only its own
epoch on graceful shutdown. The epoch combines the process start second with
the process ID, so two processes started in the same directory and second do
not collide unless the operating system also reuses a PID in that second.

Before checking a mount grant or starting a VM, the host resolves every bind
root to its canonical path and passes that same path to the backend. An empty
egress list disables the network interface; otherwise the backend installs a
default-deny policy for the granted destinations. DNS is
also explicit: only a whole-family port-53 grant such as `0.0.0.0/0:53` (or
`[::]/0:53`) enables the gateway resolver. A narrower port-53 grant does not,
so operations such as Alpine's `apk add` need the whole-family grant as well as
the relevant HTTP or HTTPS egress.

The bundled sandbox plugin has one `run` tool. Its `command` argument is an argv
array executed directly, without a shell. The plugin defaults `image` to
`docker.io/library/alpine:3.20`; every entry in `allowed_mounts` uses the
`vm.mount` scope grammar and is mounted at `/mnt<host path>` on every call, so
`ro:/path` mounts read-only and `rw:/path` mounts read-write; `allowed_egress`
supplies the network scopes. It names its VM `workspace` and reuses it across
calls within a session. Results contain the exit code, stdout, stderr, and a
note when the host's combined output cap truncated the streams.

The real backend uses microsandbox microVMs on Apple Silicon or Linux with KVM.
A system Cargo build installs the microsandbox runtime under `~/.microsandbox`
through the dependency's build script. The flake instead supplies a pinned
runtime bundle: `nix run .#chap-vm` selects the VM-enabled package, and the
`mkchap-vm-example` check exercises a declarative VM instance.

For a worked personal configuration, merge the following fragment into a
configuration that already contains an approved provider and persona. Replace
the two `/absolute/path/to/...` prefixes with real absolute paths; because this
is a personal file, component paths relative to the repository would resolve
from the personal configuration directory. The persona text shown here is the
complete hint to append to an existing persona:

```json
{
  "agent": {
    "vm": {
      "registries": ["docker.io"]
    }
  },
  "plugins": {
    "sandbox": {
      "component": "/absolute/path/to/chap/target/wasm32-wasip2/release/chap_vm_plugin.wasm",
      "settings": {
        "allowed_mounts": ["ro:/absolute/path/to/project"],
        "allowed_egress": [
          "0.0.0.0/0:443",
          "0.0.0.0/0:80",
          "0.0.0.0/0:53"
        ]
      }
    },
    "persona": {
      "component": "/absolute/path/to/chap/target/wasm32-wasip2/release/chap_persona.wasm",
      "context": {
        "channel": "system"
      },
      "settings": {
        "persona": "A reusable microVM sandbox is available through the sandbox plugin's run tool. The host project is mounted read-only at /mnt/absolute/path/to/project."
      }
    }
  }
}
```

Build the two components from the CHAP checkout, then point every command at the
personal file explicitly. This matters in the repository because its
`./chap.json` takes precedence over the default personal path. With Cargo:

```console
cargo build -p chap-vm-plugin -p chap-persona --release --target wasm32-wasip2
cargo run -p chap-cli --features vm -- --config "$HOME/.config/chap/chap.json" grants review sandbox
cargo run -p chap-cli --features vm -- --config "$HOME/.config/chap/chap.json" grants approve sandbox
cargo run -p chap-cli --features vm -- --config "$HOME/.config/chap/chap.json" plugins check
cargo run -p chap-cli --features vm -- --config "$HOME/.config/chap/chap.json"
```

Or use the VM-enabled Nix package after the same component build:

```console
nix run .#chap-vm -- --config "$HOME/.config/chap/chap.json" grants review sandbox
nix run .#chap-vm -- --config "$HOME/.config/chap/chap.json" grants approve sandbox
nix run .#chap-vm -- --config "$HOME/.config/chap/chap.json" plugins check
nix run .#chap-vm -- --config "$HOME/.config/chap/chap.json"
```

These paths use the fallback personal location; substitute
`$XDG_CONFIG_HOME/chap/chap.json` when `XDG_CONFIG_HOME` is set. The entire
stack must be compiled in with Cargo's `vm` feature or the `chap-vm` Nix
package. A build without it rejects `agent.vm` at startup and refuses plugins
that require VM permissions at admission.

The real, booting backend test is marked ignored because it needs the host
hypervisor and network access:

```console
cargo test -p chap-vm --features host,microsandbox -- --ignored
```

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

### Plugin HTTP timeouts

`agent.http.request_timeout_ceiling_ms` controls the host timeout ceiling for
HTTP requests from every plugin. It defaults to 300,000 milliseconds and accepts
integers from 1 through 600,000. Omitted sections or fields retain the default;
unknown fields are rejected.

```json
{
  "agent": {
    "http": { "request_timeout_ceiling_ms": 360000 },
    "budgets": { "provider": { "deadline_ms": 390000 } }
  }
}
```

The same ceiling applies during plugin checks, admission preflight, and admitted
plugin execution. A lower plugin-requested HTTP timeout still takes precedence.
The HTTP ceiling bounds connection, first-byte, and between-frame inactivity
timeouts. The provider invocation budget defaults to 600,000 milliseconds and
bounds the total invocation. Raising the HTTP ceiling does not change
`agent.budgets.provider.deadline_ms`. For a server that buffers its response,
waiting for the first response byte includes the server's processing time. This HTTP ceiling is not an overall agent
turn deadline.

Previously both defaults were 120,000 milliseconds. Set both fields explicitly
to 120000 to retain those limits.

### Plugin call budgets

Fuel and wall-clock deadlines are configured agent-wide by plugin role:

```json
{
  "agent": {
    "budgets": {
      "admission": { "fuel": 25000000, "deadline_ms": 30000 },
      "provider": { "fuel": 25000000, "deadline_ms": 600000 },
      "tools": { "fuel": 25000000, "deadline_ms": 30000 },
      "context": { "fuel": 25000000, "deadline_ms": 10000 }
    }
  }
}
```

The values shown are the defaults; they are initial guesses pending measurement
of realistic plugin workloads. Omitted roles or fields retain their defaults.
The `tools` budget covers both tool-definition loading and tool execution.
Every fuel value must be between 1 and 1,000,000,000 instructions, and every
`deadline_ms` must be between 1 and 600,000 milliseconds (10 minutes).

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
For the sandbox plugin, a whole-family port-53 egress scope (`0.0.0.0/0:53` or
`[::]/0:53`) explicitly enables gateway DNS; without one, name resolution
stays blocked.

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
manifest has been approved. Check that the configured components are coherent,
then review and approve each instance before starting CHAP:

```console
cargo run -- plugins check
cargo run -- grants review
cargo run -- grants approve openai
cargo run -- grants approve kagi
cargo run -- grants approve persona
```

With an exec-enabled build, configure the exec plugin's command scopes in its
settings:

```json
{
  "plugins": {
    "exec": {
      "component": "./target/wasm32-wasip2/release/chap_exec_plugin.wasm",
      "settings": {
        "allowed_commands": ["git status --short", "git commit"]
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

A prefix bounds only the initial argv tokens: it does not bound later flags,
paths, subprocesses, or side effects. Treat any program with a run-helper
interface—including `sh`, `cargo`, `make`, `npm`, bare `git`, `rg --pre`,
`find -exec`, `xargs`, `env`, `nix`, `docker`, `ssh`, `awk`, GNU `sed`, and
editors or pagers—as arbitrary execution with the operator's authority.
`git commit` can read any host file through `-F` or `--pathspec-from-file`.
Even apparently read-only commands can read any path accepted by their later
arguments, and host or repository configuration can change what they execute.
Use the VM sandbox when a microVM boundary is required.

`grants review` also accepts one instance id. `grants deny <instance-id>` removes
that instance's approval. CHAP stores approvals below `$XDG_STATE_HOME/chap`, or
`$HOME/.local/state/chap` when `XDG_STATE_HOME` is unset or empty. Named
configurations use `named/<name>/consent.json`; unnamed configurations use
`by-path/<sha256-of-absolute-config-path>/consent.json`. Concrete scopes remain
in `chap.json`. Compiled components are cached in the sibling `compiled`
directory; that cache can be deleted freely and will be rebuilt as needed. A
permission expansion, such as changing `base_url` to point at
a different origin, blocks admission until the new manifest is reviewed and
approved. Narrowing or removing authority is reported as non-blocking drift.
Approval also binds the component's exported interfaces: a plugin that starts
exporting a role it was not approved for blocks admission the same way, while
dropping a role or changing only an interface version is non-blocking.

`plugins check` verifies that every configured component exists, has matching
embedded plugin metadata, implements a supported role, publishes a schema that
accepts its settings, wires its imports, and smoke-instantiates. The check does
not require consent or enforce environment-variable presence; required variables
are reported per plugin as set or unset. Readiness remains the responsibility of
agent startup, which requires both consent and required environment variables. Use
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

`cargo run -- plugins check` exercises this consent-free preflight path. The
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
