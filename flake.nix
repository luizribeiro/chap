{
  description = "CHAP coding agent";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
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
      crane,
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
        stableRust = pkgs.rust-bin.stable.latest.default.override {
          targets = [ "wasm32-wasip2" ];
        };
        craneLib = (crane.mkLib pkgs).overrideToolchain stableRust;
        packageSrc = craneLib.path ./.;
        cargoVendorDir = craneLib.vendorCargoDeps {
          src = packageSrc;
          overrideVendorGitCheckout =
            packages: checkout:
            let
              fromGit =
                repo:
                pkgs.lib.any (
                  package: pkgs.lib.hasPrefix "git+https://github.com/${repo}" (package.source or "")
                ) packages;
            in
            if fromGit "bytecodealliance/wasmtime" then
              checkout.overrideAttrs (old: {
                # Crane inspects unpublished workspace crates whose declared
                # README files are absent from the pinned Wasmtime checkout.
                postPatch = (old.postPatch or "") + ''
                  for crate in crates/bench-api crates/c-api/artifact; do
                    [ -e "$crate/README.md" ] || cp README.md "$crate/README.md"
                  done
                '';
              })
            else if fromGit "luizribeiro/lockgate" then
              checkout.overrideAttrs (old: {
                # The proc macro reads this sibling contract after Crane has
                # extracted the git workspace into individual crate trees.
                postInstall = (old.postInstall or "") + ''
                  mkdir -p "$out/lockgate/wit"
                  cp crates/lockgate/wit/config.wit "$out/lockgate/wit/"
                '';
              })
            else
              checkout;
        };
        packageArgs = {
          pname = "chap";
          version = "0.1.0";
          src = packageSrc;
          inherit cargoVendorDir;
          cargoExtraArgs = "--locked -p chap-cli";
          doCheck = false;
        };
        cargoArtifacts = craneLib.buildDepsOnly packageArgs;
        chap = pkgs.lib.makeOverridable (
          { withExec ? false }:
          craneLib.buildPackage (
            packageArgs
            // {
              inherit cargoArtifacts;
              cargoExtraArgs =
                packageArgs.cargoExtraArgs
                + pkgs.lib.optionalString withExec " --features chap-cli/exec";
            }
          )
        ) { };
        # pname must equal the Cargo package name: the default cargoExtraArgs
        # builds `-p pname`, and the install step expects cargo's artifact
        # naming for that package (dashes become underscores).
        buildChapPlugin =
          {
            pname,
            src,
            cargoExtraArgs ? "--locked -p ${pname}",
            defaultSettings ? { },
            ...
          }@args:
          let
            componentName = builtins.replaceStrings [ "-" ] [ "_" ] pname;
            component = "${plugin}/lib/${componentName}.wasm";
            plugin = craneLib.buildPackage (
              builtins.removeAttrs args [
                "cargoExtraArgs"
                "defaultSettings"
              ]
              // {
                inherit pname src;
                cargoExtraArgs = "${cargoExtraArgs} --target wasm32-wasip2";
                doCheck = false;
                installPhaseCommand = ''
                  mkdir -p "$out/lib"
                  cp "target/wasm32-wasip2/release/${componentName}.wasm" "$out/lib/"
                '';
                passthru = (args.passthru or { }) // {
                  chapPlugin = {
                    inherit component defaultSettings;
                  };
                };
              }
            );
          in
          plugin;
        pluginCargoArtifacts = craneLib.buildDepsOnly {
          pname = "chap-plugins";
          version = "0.1.0";
          src = packageSrc;
          inherit cargoVendorDir;
          cargoExtraArgs = "--locked -p chap-openai-compatible -p chap-kagi -p chap-exec-plugin -p chap-persona --target wasm32-wasip2";
          doCheck = false;
        };
        pluginOpenaiCompatible = buildChapPlugin {
          pname = "chap-openai-compatible";
          src = packageSrc;
          inherit cargoVendorDir;
          cargoArtifacts = pluginCargoArtifacts;
        };
        pluginKagi = buildChapPlugin {
          pname = "chap-kagi";
          src = packageSrc;
          inherit cargoVendorDir;
          cargoArtifacts = pluginCargoArtifacts;
          defaultSettings.api_key_env = "KAGI_API_KEY";
        };
        pluginExec = buildChapPlugin {
          pname = "chap-exec-plugin";
          src = packageSrc;
          inherit cargoVendorDir;
          cargoArtifacts = pluginCargoArtifacts;
        };
        pluginPersona = buildChapPlugin {
          pname = "chap-persona";
          src = packageSrc;
          inherit cargoVendorDir;
          cargoArtifacts = pluginCargoArtifacts;
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
                extraArgs = "--workspace --all-targets --locked --exclude chap-openai-compatible --exclude chap-kagi";
                offline = false;
              };
            };
            plugin-clippy = {
              enable = true;
              name = "cargo clippy (WASI plugins)";
              entry = "${rust}/bin/cargo clippy -p chap-openai-compatible -p chap-exec-plugin -p chap-kagi --target wasm32-wasip2 --all-targets --locked -- -D warnings";
              files = "(^|/)(Cargo\\.toml|\\.cargo/config\\.toml|.*\\.rs)$";
              pass_filenames = false;
            };
            exec-clippy = {
              enable = true;
              name = "cargo clippy (exec feature)";
              entry = "${rust}/bin/cargo clippy -p chap-core --features exec --all-targets --locked -- -D warnings";
              files = "(^|/)(Cargo\\.toml|\\.cargo/config\\.toml|.*\\.rs)$";
              pass_filenames = false;
            };
            cargo-test = {
              enable = true;
              name = "cargo test";
              entry = "${rust}/bin/cargo test --workspace --all-targets --locked --exclude chap-openai-compatible --exclude chap-exec-plugin --exclude chap-kagi";
              files = "(^|/)(Cargo\\.toml|\\.cargo/config\\.toml|.*\\.rs)$";
              pass_filenames = false;
              stages = [ "pre-push" ];
            };
            plugin-test = {
              enable = true;
              name = "cargo test (WASI plugins)";
              entry = "env CARGO_TARGET_WASM32_WASIP2_RUNNER='${pkgs.wasmtime}/bin/wasmtime run' ${rust}/bin/cargo test -p chap-openai-compatible -p chap-kagi --target wasm32-wasip2 --locked";
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
        # Tests compile WASM components in-process, so Clippy and tests remain
        # in the development shell's commit and push hooks.
        packages = {
          inherit chap;
          chap-exec = chap.override { withExec = true; };
          default = chap;
          plugin-openai-compatible = pluginOpenaiCompatible;
          plugin-kagi = pluginKagi;
          plugin-exec = pluginExec;
          plugin-persona = pluginPersona;
        };

        checks = {
          formatting = formattingCheck;
          chap = chap;
          plugin-openai-compatible = pluginOpenaiCompatible;
          plugin-kagi = pluginKagi;
          plugin-exec = pluginExec;
          plugin-persona = pluginPersona;
        };

        lib = {
          inherit buildChapPlugin;
        };

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
