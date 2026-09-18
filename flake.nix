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
      self,
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
        witVersionMatch = builtins.match ".*package chap:agent@([0-9]+\\.[0-9]+\\.[0-9]+);.*" (
          builtins.readFile ./crates/chap-wit/wit/worlds.wit
        );
        witVersion =
          if witVersionMatch == null then
            throw "crates/chap-wit/wit/worlds.wit does not declare a versioned chap:agent package"
          else
            builtins.elemAt witVersionMatch 0;
        cargoVendorDir = craneLib.vendorCargoDeps {
          src = packageSrc;
        };
        microsandboxVersion = "0.7.2";
        microsandboxRuntimes = {
          aarch64-darwin = {
            bundle = "microsandbox-darwin-aarch64.tar.gz";
            bundleHash = "sha256-FKWRDGs5Xp1Q4AHYE4jL5TZvGBZcV+3kpe2j4zFbfb0=";
            libFile = "libkrunfw.5.dylib";
          };
          x86_64-linux = {
            bundle = "microsandbox-linux-x86_64.tar.gz";
            bundleHash = "sha256-R8Ij4+9SmKvwX0ftn4eYEQbkANmbs/HQQtTWiBNGsYs=";
            libFile = "libkrunfw.so.5.6.1";
          };
          aarch64-linux = {
            bundle = "microsandbox-linux-aarch64.tar.gz";
            bundleHash = "sha256-1N54FBR7g1pLmeUeI2yLMzNACR5XJ6C0XX64R9VrAio=";
            libFile = "libkrunfw.so.5.6.1";
          };
        };
        microsandboxRuntime = microsandboxRuntimes.${system} or null;
        microsandboxBundle =
          if microsandboxRuntime == null then
            null
          else
            pkgs.fetchurl {
              url = "https://github.com/superradcompany/microsandbox/releases/download/v${microsandboxVersion}/${microsandboxRuntime.bundle}";
              hash = microsandboxRuntime.bundleHash;
            };
        microsandboxHome =
          if microsandboxRuntime == null then
            null
          else
            pkgs.runCommand "microsandbox-runtime-${microsandboxVersion}" { } ''
              mkdir -p "$out/bin" "$out/lib"
              tar -xzf ${microsandboxBundle} -C "$out"
              mv "$out/msb" "$out/bin/msb"
              mv "$out/${microsandboxRuntime.libFile}" "$out/lib/${microsandboxRuntime.libFile}"
            '';
        packageArgs = {
          pname = "chap";
          version = "0.1.0";
          src = packageSrc;
          inherit cargoVendorDir;
          cargoExtraArgs = "--locked -p chap-cli";
          doCheck = false;
          buildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [ pkgs.libcap_ng ];
        };
        cargoArtifacts = craneLib.buildDepsOnly packageArgs;
        chap = pkgs.lib.makeOverridable (
          { withExec ? false, withState ? false, withVm ? false }:
          craneLib.buildPackage (
            packageArgs
            // {
              inherit cargoArtifacts;
              cargoExtraArgs =
                packageArgs.cargoExtraArgs
                + pkgs.lib.optionalString withExec " --features chap-cli/exec"
                + pkgs.lib.optionalString withState " --features chap-cli/state"
                + pkgs.lib.optionalString withVm " --features chap-cli/vm";
              nativeBuildInputs = pkgs.lib.optionals (withVm && microsandboxHome != null) [
                pkgs.makeWrapper
              ];
              postInstall = pkgs.lib.optionalString (withVm && microsandboxHome != null) ''
                wrapProgram "$out/bin/chap" \
                  --set-default MSB_PATH ${pkgs.lib.escapeShellArg "${microsandboxHome}/bin/msb"} \
                  --set-default MSB_LIBKRUNFW_PATH ${pkgs.lib.escapeShellArg "${microsandboxHome}/lib/${microsandboxRuntime.libFile}"}
              '';
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
            cargoVendorDir ? craneLib.vendorCargoDeps {
              inherit src;
            },
            defaultSettings ? { },
            ...
          }@args:
          let
            componentName = builtins.replaceStrings [ "-" ] [ "_" ] pname;
            component = "${plugin}/lib/${componentName}.wasm";
            witVersionFile = "${plugin}/share/chap-plugin/wit-version";
            plugin = craneLib.buildPackage (
              builtins.removeAttrs args [
                "cargoExtraArgs"
                "defaultSettings"
              ]
              // {
                inherit pname src cargoVendorDir;
                cargoExtraArgs = "${cargoExtraArgs} --target wasm32-wasip2";
                doCheck = false;
                nativeBuildInputs = (args.nativeBuildInputs or [ ]) ++ [ pkgs.wasm-tools ];
                installPhaseCommand = ''
                  mkdir -p "$out/lib"
                  cp "target/wasm32-wasip2/release/${componentName}.wasm" "$out/lib/"

                  witVersions=$(
                    wasm-tools component wit "$out/lib/${componentName}.wasm" \
                      | grep -Eo 'chap:agent@[0-9]+\.[0-9]+\.[0-9]+' \
                      | cut -d@ -f2 \
                      | sort -u \
                      || true
                  )
                  if [ -z "$witVersions" ]; then
                    echo "component does not import or export a versioned chap:agent package" >&2
                    exit 1
                  fi
                  if [ "$(printf '%s\n' "$witVersions" | wc -l | tr -d ' ')" -ne 1 ]; then
                    echo "component references multiple chap:agent versions:" >&2
                    printf '%s\n' "$witVersions" >&2
                    exit 1
                  fi

                  mkdir -p "$out/share/chap-plugin"
                  printf '%s\n' "$witVersions" > "$out/share/chap-plugin/wit-version"
                '';
                passthru = (args.passthru or { }) // {
                  chapPlugin = {
                    inherit component defaultSettings witVersionFile;
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
          cargoExtraArgs = "--locked -p chap-openai-compatible -p chap-kagi -p chap-exec-plugin -p chap-state-plugin -p chap-workspace-plugin -p chap-persona --target wasm32-wasip2";
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
        pluginState = buildChapPlugin {
          pname = "chap-state-plugin";
          src = packageSrc;
          inherit cargoVendorDir;
          cargoArtifacts = pluginCargoArtifacts;
        };
        pluginWorkspace = buildChapPlugin {
          pname = "chap-workspace-plugin";
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
        # Exercises buildChapPlugin's default vendoring, the path third-party
        # builds rely on; every other in-tree package passes cargoVendorDir.
        pluginDefaultVendor = buildChapPlugin {
          pname = "chap-persona";
          src = packageSrc;
          cargoArtifacts = pluginCargoArtifacts;
        };
        # The scaffold intentionally has no lockfile. Give its check the local
        # workspace lock and SDK without changing the files users receive.
        templatePluginSrc = pkgs.runCommand "chap-template-plugin-source" { } ''
          mkdir "$out"
          cp -R ${./nix/templates/plugin} "$out/plugin"
          chmod -R u+w "$out"
          cp ${./Cargo.lock} "$out/plugin/Cargo.lock"
          ln -s ${packageSrc} "$out/chap"
          substituteInPlace "$out/plugin/Cargo.toml" \
            --replace-fail \
              'chap-plugin = { git = "https://github.com/luizribeiro/chap.git", default-features = false }' \
              'chap-plugin = { path = "../chap/crates/chap-plugin", default-features = false }'
        '';
        templatePlugin = buildChapPlugin {
          pname = "my-plugin";
          src = templatePluginSrc;
          cargoToml = "${templatePluginSrc}/plugin/Cargo.toml";
          sourceRoot = "chap-template-plugin-source/plugin";
          inherit cargoVendorDir;
          cargoArtifacts = null;
          cargoExtraArgs = "-p my-plugin";
        };
        templatePluginFlake = (import ./nix/templates/plugin/flake.nix).outputs { chap = self; };
        templateWitVersion =
          assert pkgs.lib.assertMsg (templatePluginFlake.lib.witVersion == witVersion)
            "plugin template WIT version ${templatePluginFlake.lib.witVersion} does not match ${witVersion}";
          pkgs.runCommand "chap-template-wit-version" { } "touch $out";
        pluginRoleSettings = spec:
          pkgs.lib.optionalAttrs ((spec.tools or null) != null) { inherit (spec) tools; }
          // pkgs.lib.optionalAttrs ((spec.context or null) != null) { inherit (spec) context; };
        normalizeChapPlugin =
          id: spec:
          let
            fromPlugin = source:
              let
                plugin = source.plugin;
                metadata =
                  assert pkgs.lib.assertMsg (pkgs.lib.isDerivation plugin) "mkChap: plugin `${id}`'s `plugin` must be a derivation";
                  assert pkgs.lib.assertMsg (plugin ? chapPlugin) "mkChap: plugin `${id}`'s derivation must provide `passthru.chapPlugin`";
                  plugin.chapPlugin;
              in
              {
                config = {
                  component = metadata.component;
                  settings = (metadata.defaultSettings or { }) // (source.settings or { });
                }
                // pluginRoleSettings source;
                witVersionFile = metadata.witVersionFile or null;
              };
          in
          if pkgs.lib.isDerivation spec then
            fromPlugin { plugin = spec; }
          else if !builtins.isAttrs spec then
            throw "mkChap: plugin `${id}` must be a plugin derivation or an attribute set"
          else if spec ? plugin then
            assert pkgs.lib.assertMsg (!(spec ? component)) "mkChap: plugin `${id}` cannot define both `plugin` and `component`";
            fromPlugin spec
          else if spec ? component then
            assert pkgs.lib.assertMsg (builtins.isPath spec.component || builtins.isString spec.component)
              "mkChap: plugin `${id}`'s `component` must be a path or string";
            {
              config = {
                inherit (spec) component;
                settings = spec.settings or { };
              }
              // pluginRoleSettings spec;
              witVersionFile = null;
            }
          else
            throw "mkChap: plugin `${id}` must define either `plugin` or `component`";
        mkChap =
          {
            name,
            settings ? { },
            plugins ? { },
            package ? chap,
          }:
          let
            normalizedPlugins = pkgs.lib.mapAttrs normalizeChapPlugin plugins;
            pluginIds = builtins.attrNames normalizedPlugins;
            config = { inherit name; } // settings // { plugins = pkgs.lib.mapAttrs (_: normalized: normalized.config) normalizedPlugins; };
            configFile = pkgs.writeText "chap-${name}.json" (builtins.toJSON config);
            chapBinary = "${package}/bin/chap";
            witChecks = pkgs.lib.concatMapStringsSep "\n" (
              id:
              let
                normalized = normalizedPlugins.${id};
                # Plugin packages validated and recorded their WIT version at
                # build time; only raw components need extraction here.
                readVersions =
                  if normalized.witVersionFile != null then
                    ''
                      actual_wit_versions="$(cat ${pkgs.lib.escapeShellArg (toString normalized.witVersionFile)})"
                    ''
                  else
                    ''
                      actual_wit_versions="$(
                        wasm-tools component wit ${pkgs.lib.escapeShellArg (toString normalized.config.component)} \
                          | grep -Eo 'chap:agent@[0-9]+\.[0-9]+\.[0-9]+' \
                          | cut -d@ -f2 \
                          | sort -u \
                          || true
                      )"
                    '';
              in
              ''
                plugin_id=${pkgs.lib.escapeShellArg id}
                ${readVersions}
                if [ "$actual_wit_versions" != "$expected_wit_version" ]; then
                  displayed_wit_versions="$(printf '%s\n' "$actual_wit_versions" | paste -sd, -)"
                  [ -n "$displayed_wit_versions" ] || displayed_wit_versions='<none>'
                  printf 'mkChap: plugin "%s" WIT version mismatch: component references "%s", CHAP expects "%s"\n' \
                    "$plugin_id" "$displayed_wit_versions" "$expected_wit_version" >&2
                  exit 1
                fi
              ''
            ) pluginIds;
          in
          assert pkgs.lib.assertMsg (!(settings ? name)) "mkChap: `settings` must not define `name`; use the top-level `name` argument";
          assert pkgs.lib.assertMsg (!(settings ? plugins)) "mkChap: `settings` must not define `plugins`; use the top-level `plugins` argument";
          pkgs.stdenvNoCC.mkDerivation {
            name = "chap-${name}";
            dontUnpack = true;
            nativeBuildInputs = [
              pkgs.makeWrapper
              pkgs.wasm-tools
            ];

            buildPhase = ''
              runHook preBuild

              expected_wit_version=${pkgs.lib.escapeShellArg witVersion}
              ${witChecks}

              ${pkgs.lib.escapeShellArg chapBinary} --config ${pkgs.lib.escapeShellArg configFile} plugins check

              runHook postBuild
            '';

            installPhase = ''
              runHook preInstall

              mkdir -p "$out/etc" "$out/bin"
              cp ${pkgs.lib.escapeShellArg configFile} "$out/etc/chap.json"
              makeWrapper ${pkgs.lib.escapeShellArg chapBinary} "$out/bin/chap" \
                --set-default CHAP_CONFIG "$out/etc/chap.json"

              runHook postInstall
            '';

            meta.mainProgram = "chap";
          };
        mkChapExample = mkChap {
          name = "example";
          plugins = {
            openai = {
              plugin = pluginOpenaiCompatible;
              settings = {
                base_url = "http://127.0.0.1:8080/v1";
                model = "example-model";
              };
            };
            kagi = {
              plugin = pluginKagi;
              settings.api_key_env = "KAGI_API_KEY";
            };
            persona = {
              plugin = pluginPersona;
              settings.persona = "Be concise and practical.";
            };
          };
        };
        mkChapExecExample = mkChap {
          name = "example-exec";
          package = chap.override { withExec = true; };
          settings.agent.exec = { };
          plugins.exec = {
            plugin = pluginExec;
            settings.allowed_commands = [ "echo" ];
          };
        };
        mkChapStateExample = mkChap {
          name = "example-state";
          package = chap.override { withState = true; };
          settings.agent.state = { };
          plugins.memory = {
            plugin = pluginState;
          };
        };
        mkChapVmExample = mkChap {
          name = "example-vm";
          package = chap.override { withVm = true; };
          settings.agent.vm.registries = [ "docker.io" ];
          settings.agent.workspace = {
            image = "docker.io/library/alpine:3.20";
            mount = "ro";
            egress = [ "127.0.0.1:1" ];
          };
          plugins.workspace.plugin = pluginWorkspace;
        };
        mkChapFormsExample = mkChap {
          name = "example-forms";
          plugins = {
            kagi = pluginKagi;
            persona = {
              component = "${pluginPersona}/lib/chap_persona.wasm";
              settings.persona = "Be concise and practical.";
            };
          };
        };
        mkChapEvalGuards =
          let
            rejects = args: !(builtins.tryEval (mkChap args).drvPath).success;
            guards = {
              non-attrset-spec = rejects {
                name = "guard";
                plugins.bad = 42;
              };
              empty-spec = rejects {
                name = "guard";
                plugins.bad = { };
              };
              plugin-and-component = rejects {
                name = "guard";
                plugins.bad = {
                  plugin = pluginPersona;
                  component = "/component.wasm";
                };
              };
              plugin-not-derivation = rejects {
                name = "guard";
                plugins.bad.plugin = 42;
              };
              plugin-without-passthru = rejects {
                name = "guard";
                plugins.bad.plugin = pkgs.hello;
              };
              component-wrong-type = rejects {
                name = "guard";
                plugins.bad.component = 42;
              };
              settings-name-clobber = rejects {
                name = "guard";
                settings.name = "other";
              };
              settings-plugins-clobber = rejects {
                name = "guard";
                settings.plugins = { };
              };
            };
            accepted = builtins.attrNames (pkgs.lib.filterAttrs (_: rejected: !rejected) guards);
          in
          if accepted == [ ] then
            pkgs.runCommand "mkchap-eval-guards" { } "touch $out"
          else
            throw "mkChap accepted invalid arguments: ${builtins.concatStringsSep ", " accepted}";
        templateInstance =
          ((import ./nix/templates/instance/flake.nix).outputs { chap = self; }).packages.${system}.default;
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
              entry = "${rust}/bin/cargo clippy -p chap-openai-compatible -p chap-exec-plugin -p chap-state-plugin -p chap-workspace-plugin -p chap-kagi --target wasm32-wasip2 --all-targets --locked -- -D warnings";
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
            state-clippy = {
              enable = true;
              name = "cargo clippy (state feature)";
              entry = "${rust}/bin/cargo clippy -p chap-core --features state --all-targets --locked -- -D warnings";
              files = "(^|/)(Cargo\\.toml|\\.cargo/config\\.toml|.*\\.rs)$";
              pass_filenames = false;
            };
            vm-clippy = {
              enable = true;
              name = "cargo clippy (vm feature)";
              entry = "${rust}/bin/cargo clippy -p chap-core --features vm --all-targets --locked -- -D warnings";
              files = "(^|/)(Cargo\\.toml|\\.cargo/config\\.toml|.*\\.rs)$";
              pass_filenames = false;
            };
            microsandbox-vm-clippy = {
              enable = true;
              name = "cargo clippy (microsandbox vm backend)";
              entry = "${rust}/bin/cargo clippy -p chap-vm --features host,microsandbox --all-targets --locked -- -D warnings";
              files = "(^|/)(Cargo\\.toml|\\.cargo/config\\.toml|.*\\.rs)$";
              pass_filenames = false;
            };
            all-capabilities-clippy = {
              enable = true;
              name = "cargo clippy (all capabilities)";
              entry = "${rust}/bin/cargo clippy -p chap-core --features exec,state,vm --all-targets --locked -- -D warnings";
              files = "(^|/)(Cargo\\.toml|\\.cargo/config\\.toml|.*\\.rs)$";
              pass_filenames = false;
            };
            cargo-test = {
              enable = true;
              name = "cargo test";
              entry = "${rust}/bin/cargo test --workspace --all-targets --locked --exclude chap-openai-compatible --exclude chap-exec-plugin --exclude chap-state-plugin --exclude chap-workspace-plugin --exclude chap-kagi";
              files = "(^|/)(Cargo\\.toml|\\.cargo/config\\.toml|.*\\.rs)$";
              pass_filenames = false;
              stages = [ "pre-push" ];
            };
            microsandbox-vm-test = {
              enable = true;
              name = "cargo test (microsandbox vm backend)";
              entry = "${rust}/bin/cargo test -p chap-vm --features host,microsandbox --locked";
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
          chap-state = chap.override { withState = true; };
          chap-vm = chap.override { withVm = true; };
          default = chap;
          plugin-openai-compatible = pluginOpenaiCompatible;
          plugin-kagi = pluginKagi;
          plugin-exec = pluginExec;
          plugin-state = pluginState;
          plugin-workspace = pluginWorkspace;
          plugin-persona = pluginPersona;
          mkchap-vm-example = mkChapVmExample;
          mkchap-forms-example = mkChapFormsExample;
        };

        checks = {
          formatting = formattingCheck;
          chap = chap;
          plugin-openai-compatible = pluginOpenaiCompatible;
          plugin-kagi = pluginKagi;
          plugin-exec = pluginExec;
          plugin-state = pluginState;
          plugin-workspace = pluginWorkspace;
          plugin-persona = pluginPersona;
          plugin-default-vendor = pluginDefaultVendor;
          template-plugin = templatePlugin;
          template-instance = templateInstance;
          template-wit-version = templateWitVersion;
          mkchap-example = mkChapExample;
          mkchap-exec-example = mkChapExecExample;
          mkchap-state-example = mkChapStateExample;
          mkchap-vm-example = mkChapVmExample;
          mkchap-forms-example = mkChapFormsExample;
          mkchap-eval-guards = mkChapEvalGuards;
        };

        lib = {
          inherit buildChapPlugin mkChap witVersion;
        };

        devShells.default = pkgs.mkShell {
          packages = [
            rust
            pkgs.wasmtime
          ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux [ pkgs.libcap_ng ] ++ gitHooks.enabledPackages;

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
    )
    // {
      templates = rec {
        plugin = {
          path = ./nix/templates/plugin;
          description = "A third-party CHAP tools plugin";
        };
        instance = {
          path = ./nix/templates/instance;
          description = "A declarative CHAP instance";
        };
        default = instance;
      };
    };
}
