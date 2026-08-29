{
  description = "A CHAP tools plugin";

  inputs.chap.url = "github:luizribeiro/chap";

  outputs =
    { chap, ... }:
    chap.inputs.flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import chap.inputs.nixpkgs {
          inherit system;
          overlays = [ chap.inputs.rust-overlay.overlays.default ];
        };
        rust = pkgs.rust-bin.stable.latest.default.override {
          targets = [ "wasm32-wasip2" ];
        };
      in
      {
        packages.default = chap.lib.${system}.buildChapPlugin {
          pname = "my-plugin";
          src = ./.;
        };

        devShells.default = pkgs.mkShell {
          packages = [
            rust
            pkgs.wasm-tools
            pkgs.wasmtime
          ];

          CARGO_TARGET_WASM32_WASIP2_RUNNER = "wasmtime run";
        };
      }
    );
}
