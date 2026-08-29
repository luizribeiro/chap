# Declarative CHAP instance

Initialize this template in a new directory and build it:

```console
nix flake init -t github:luizribeiro/chap#instance
nix build
```

Run the configured CHAP package directly from the flake:

```console
nix run . -- plugins list
```

Consent approvals are stored per user in the XDG state directory. Inspect and
grant them with `chap grants review` and `chap grants approve <plugin-id>`.

Nix settings are world-readable in the store. Put only environment-variable
names such as `api_key_env` in settings, and pass secrets through those
environment variables.
