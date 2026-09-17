# Nix

This guide covers running CHAP through the flake and declaring your own instance with `mkChap`.

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

## Templates

`nix flake init -t github:luizribeiro/chap#instance` creates a minimal
declarative instance flake. Plugin authors can use
`nix flake init -t github:luizribeiro/chap#plugin` for a working tools-plugin
skeleton; generate its `Cargo.lock` before the first Nix build.
