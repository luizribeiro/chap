# Plugins

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

CHAP resolves the workspace directory separately, in this order:

1. The path passed to `--workspace`.
2. The path in `CHAP_WORKSPACE`.
3. The directory where CHAP was started.

The selected directory is canonicalized before the agent starts.

The repository includes an OpenAI-compatible Chat Completions provider, Kagi
web tools, a persona context contributor, and exec, state, and workspace tools.
The exec plugin exposes an argv-style host process tool mediated by
command-prefix grants; the workspace plugin runs shell commands in a microVM.

Build the configured release components with the system Cargo:

```console
cargo build -p chap-openai-compatible -p chap-exec-plugin \
  -p chap-state-plugin -p chap-workspace-plugin -p chap-kagi \
  -p chap-persona --release --target wasm32-wasip2
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
the startup and shutdown reaps; `exec_timeout_ceiling_ms` caps a command's
`timeout-ms`; `exec_max_output_bytes` caps each stream separately, keeping its
head and tail with an omission marker between them; and
`read_file_max_bytes` caps one file read.

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
- `vm.manage` with the `created-by-caller` scope permits get, exec, file reads,
  file writes, and destroy for VMs created by that caller. The `workspace`
  scope permits access to the host-owned workspace VM instead.

Caller-owned VM identities include the plugin instance and session. Each
plugin can see and manage only its own namespace, even when another plugin uses
the same logical name. Names beginning with `@` are reserved for host-owned
VMs, so plugins cannot create them. The reserved `@workspace` handle identifies
the agent's workspace VM and cannot be destroyed by a plugin.

The optional `agent.workspace` section defines that host-owned VM:

```json
{
  "agent": {
    "vm": {
      "registries": ["docker.io"]
    },
    "workspace": {
      "image": "docker.io/library/rust:1-alpine",
      "mount": "rw",
      "egress": [
        "0.0.0.0/0:443",
        "0.0.0.0/0:80",
        "0.0.0.0/0:53"
      ],
      "secrets": {
        "GITHUB_TOKEN": {
          "from_env": "CHAP_GITHUB_TOKEN",
          "hosts": ["api.github.com", "github.com"]
        }
      }
    }
  }
}
```

`image` is required and follows the registry policy in `agent.vm`. `mount` is
required and is either `"ro"` or `"rw"`. `egress` uses the same scopes as
`vm.egress` and defaults to an empty list. The directory selected by
`--workspace`, `CHAP_WORKSPACE`, or the startup directory is mounted at
`/mnt/workspace` with the configured mode.

`secrets` maps a guest environment variable to a host environment variable and
an exact hostname allowlist. It defaults to an empty map. `plugins check`
validates the names and hosts without requiring source variables to be present.
At agent startup, CHAP checks that each source variable is set and non-empty,
but never reads, stores, hashes, logs, or prints its value. The microsandbox SDK
reads the value from the host environment itself. The guest receives a
placeholder, not the value, and the network gateway substitutes the value only
in requests to an allowed host. A request that carries the placeholder to any
other host is blocked. The VM configuration hash covers the guest name,
source-variable name, and hosts, never the value. VMs created by plugins through
`vm.create` do not receive workspace secrets.

Once any secret is configured, the sandbox gateway terminates TLS for every
host, although substitution remains limited to that secret's allowlist. The
sandbox installs its CA in the guest trust store and sets the common TLS trust
environment variables before commands run, so no manual CA setup is needed in
images with a normal trust-store layout.

For GitHub access, the image must contain `gh` and `git`; on Alpine, install the
`github-cli` and `git` packages. Run `gh auth setup-git` once in the workspace
VM so Git uses the credential helper. Prefer a fine-grained GitHub token whose
repository access and permissions are limited to the repository being used.

There is one workspace VM per agent process. It boots lazily on first use, is
shared by approved plugins, and does not count against
`limits.max_vms_per_plugin`. The backend labels every VM with an installation
and process epoch, reaps older epochs for that installation at startup, and
destroys the current epoch on graceful shutdown. The epoch combines the process
start second with the process ID.

The guarded `workspace` import returns the reserved handle plus its image,
guest mount, and egress metadata without booting the VM. Calling it requires
`vm.manage` with the `workspace` scope. The SDK exposes the import as
`Vm::workspace`; the returned handle supports the normal exec and file calls.
The workspace record also carries the effective command time limit: the smaller
of `agent.budgets.tools.deadline_ms` and
`agent.vm.calls.exec_timeout_ceiling_ms`.
An exec result's `exit-code` is absent when the host killed the command at its
deadline.

Before checking a mount grant or starting a VM, the host resolves every bind
root to its canonical path and passes that same path to the backend. An empty
egress list disables the network interface; otherwise the backend installs a
default-deny policy for the configured destinations. DNS is also explicit:
only a whole-family port-53 scope such as `0.0.0.0/0:53` or `[::]/0:53` enables
the gateway resolver. A narrower port-53 scope does not, so operations such as
Alpine's `apk add` need the whole-family scope as well as the relevant HTTP or
HTTPS egress.

Address grants admit more than they read. A scope allows every service reachable
at that address and port, so a grant meant for one API on a shared host or CDN
also reaches every other site behind the same address, and `grants review` can
only show the address, not what it serves. The whole-family port-53 scope that
enables DNS also lets the guest reach any DNS server on the internet and resolve
any name, and both of those can carry data out of the VM. Name-based egress
scopes that close both gaps are tracked in
[chap#43](https://github.com/luizribeiro/chap/issues/43). Until then, prefer
specific addresses over wide CIDRs and treat the DNS grant as network access,
not just name resolution.

The bundled workspace plugin has no settings and requests exactly `vm.manage`
with the `workspace` scope. Its one `run` tool accepts a command string and
executes it with `sh -c`, with stderr merged into stdout in order, from the
workspace mount in the reused VM. The tool description reports the working
directory, image, guest mount mode, egress scopes, reuse behavior, effective
time limit, and how capped output is rendered. Results contain the exit code,
wall time, and command output. When the output exceeds the host's cap, its
beginning and end are separated by an omission marker. A command killed at its deadline returns what it printed with a
timeout line in place of the exit code.
The optional integer `timeout_secs` defaults to 120 seconds or the effective
limit when lower, and requests above that limit are clamped.

The real backend uses microsandbox microVMs on Apple Silicon or Linux with KVM.
A system Cargo build installs the microsandbox runtime under `~/.microsandbox`
through the dependency's build script. The flake instead supplies a pinned
runtime bundle: `nix run .#chap-vm` selects the VM-enabled package, and the
`mkchap-vm-example` check exercises a declarative VM instance.

For a worked personal configuration, merge the following fragment into a
configuration that already contains an approved provider and persona. Copy the
built components to the paths shown or replace those paths with their installed
locations. The project directory stays out of the personal file; select it with
`--workspace`, `CHAP_WORKSPACE`, or the directory where CHAP starts. The
persona text shown here is the complete hint to append to an existing persona:

```json
{
  "agent": {
    "vm": {
      "registries": ["docker.io"]
    },
    "workspace": {
      "image": "docker.io/library/alpine:3.20",
      "mount": "ro",
      "egress": [
        "0.0.0.0/0:443",
        "0.0.0.0/0:80",
        "0.0.0.0/0:53"
      ]
    }
  },
  "plugins": {
    "workspace": {
      "component": "/opt/chap/chap_workspace_plugin.wasm"
    },
    "persona": {
      "component": "/opt/chap/chap_persona.wasm",
      "context": {
        "channel": "system"
      },
      "settings": {
        "persona": "Use the workspace run tool for files in /mnt/workspace."
      }
    }
  }
}
```

Build the two components from the CHAP checkout, then point every command at the
personal file explicitly. This matters in the repository because its
`./chap.json` takes precedence over the default personal path. With Cargo:

```console
config="$HOME/.config/chap/chap.json"
cargo build -p chap-workspace-plugin -p chap-persona \
  --release --target wasm32-wasip2
cargo run -p chap-cli --features vm -- --workspace . --config "$config" \
  grants review workspace
cargo run -p chap-cli --features vm -- --workspace . --config "$config" \
  grants approve workspace
cargo run -p chap-cli --features vm -- --workspace . --config "$config" \
  plugins check
cargo run -p chap-cli --features vm -- --workspace . --config "$config"
```

Or use the VM-enabled Nix package after the same component build:

```console
config="$HOME/.config/chap/chap.json"
nix run .#chap-vm -- --workspace . --config "$config" \
  grants review workspace
nix run .#chap-vm -- --workspace . --config "$config" \
  grants approve workspace
nix run .#chap-vm -- --workspace . --config "$config" plugins check
nix run .#chap-vm -- --workspace . --config "$config"
```

These paths use the fallback personal location; substitute
`$XDG_CONFIG_HOME/chap/chap.json` when `XDG_CONFIG_HOME` is set. The entire
stack must be compiled in with Cargo's `vm` feature or the `chap-vm` Nix
package. A build without it rejects `agent.vm` and `agent.workspace` at startup
and refuses plugins that require VM permissions at admission.

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
      "provider": { "fuel": 1000000000, "deadline_ms": 600000 },
      "tools": { "fuel": 1000000000, "deadline_ms": 30000 },
      "context": { "fuel": 25000000, "deadline_ms": 10000 }
    }
  }
}
```

The provider and tools roles use the full fuel ceiling because their work scales
with conversation history and tool input. Admission and context calls do fixed
work and use smaller fuel budgets. Omitted roles or fields retain their defaults.
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
For `agent.workspace`, a whole-family port-53 egress scope such as
`0.0.0.0/0:53` or `[::]/0:53` explicitly enables gateway DNS. Without one,
name resolution stays blocked.

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
[OpenAI-compatible provider](../plugins/openai-compatible/src/lib.rs) and [Kagi
tools](../plugins/kagi/src/lib.rs), along with the [Persona context
plugin](../plugins/persona/src/lib.rs), do:

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
contracts](../crates/chap-wit/wit/) for the CHAP-owned role interfaces;
Lockgate adds its configuration interfaces automatically.
