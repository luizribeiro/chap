{
  description = "SAGE coding agent";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    git-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      rust-overlay,
      git-hooks,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        rust = pkgs.rust-bin.nightly.latest.default.override {
          extensions = [
            "clippy"
            "rust-analyzer"
            "rust-src"
            "rustfmt"
          ];
          targets = [ "wasm32-wasip2" ];
        };
        wasiSysroot = import ./nix/wasip3-sysroot.nix { inherit pkgs system; };
        rustfmtHook = {
          enable = true;
          packageOverrides = {
            cargo = rust;
            rustfmt = rust;
          };
          settings.check = true;
        };
        gitHooks = git-hooks.lib.${system}.run {
          src = ./.;
          hooks = {
            rustfmt = rustfmtHook;
            clippy = {
              enable = true;
              files = "(^|/)(Cargo\\.toml|\\.cargo/config\\.toml|.*\\.rs)$";
              packageOverrides = {
                cargo = rust;
                clippy = rust;
              };
              settings = {
                denyWarnings = true;
                extraArgs = "--workspace --all-targets --locked --exclude sage-openai-compatible --exclude sage-kagi";
                offline = false;
              };
            };
            plugin-clippy = {
              enable = true;
              name = "cargo clippy (WASI plugins)";
              entry = "${rust}/bin/cargo clippy -p sage-openai-compatible -p sage-kagi --target wasm32-wasip2 --all-targets --locked -- -D warnings";
              files = "(^|/)(Cargo\\.toml|\\.cargo/config\\.toml|.*\\.rs)$";
              pass_filenames = false;
            };
            cargo-test = {
              enable = true;
              name = "cargo test";
              entry = "${rust}/bin/cargo test --workspace --all-targets --locked --exclude sage-openai-compatible --exclude sage-kagi";
              files = "(^|/)(Cargo\\.toml|\\.cargo/config\\.toml|.*\\.rs)$";
              pass_filenames = false;
              stages = [ "pre-push" ];
            };
            plugin-test = {
              enable = true;
              name = "cargo test (WASI plugins)";
              entry = "env CARGO_TARGET_WASM32_WASIP2_RUNNER='${pkgs.wasmtime}/bin/wasmtime run' ${rust}/bin/cargo test -p sage-openai-compatible -p sage-kagi --target wasm32-wasip2 --locked";
              files = "(^|/)(Cargo\\.toml|\\.cargo/config\\.toml|.*\\.rs)$";
              pass_filenames = false;
              stages = [ "pre-push" ];
            };
          };
        };
        formattingCheck = git-hooks.lib.${system}.run {
          src = ./.;
          hooks.rustfmt = rustfmtHook;
        };
      in
      {
        # Cargo's Git dependencies are unavailable in the Nix build sandbox, so
        # the sandboxed check is limited to formatting. Clippy and tests run in
        # the development shell's commit and push hooks instead.
        checks.formatting = formattingCheck;

        devShells.default = pkgs.mkShell {
          packages = [
            rust
            pkgs.wasmtime
          ] ++ gitHooks.enabledPackages;

          shellHook = gitHooks.shellHook;

          RUST_BACKTRACE = "1";
          CARGO_TARGET_WASM32_WASIP3_RUSTFLAGS =
            if wasiSysroot == null then
              ""
            else
              "-Lnative=${wasiSysroot}/lib/wasm32-wasip3 -Clink-arg=${wasiSysroot}/lib/wasm32-wasip3/__cabi_realloc_wrapper.o -Clink-arg=-lc -Clink-arg=--export=__wasm_init_task -Clink-arg=--export=__wasm_init_async_task";

          # Rust's WASIp2 test harness uses its self-contained sysroot. An
          # external WASI SDK path leaves command components with unresolved
          # thread initialization symbols.
          CARGO_TARGET_WASM32_WASIP2_RUSTFLAGS = "";
        };
      }
    );
}
