# CHAP tools plugin

Enter the development shell and create the lockfile required by the first Nix
build:

```console
nix develop
cargo generate-lockfile
```

Build the component directly with Cargo or through Nix, and run the tests
under the development shell's Wasmtime runner:

```console
cargo build --release --target wasm32-wasip2
cargo test --target wasm32-wasip2
nix build
```

When this flake is an input named `greeting` in a CHAP instance, pass its plugin
package to `mkChap`:

```nix
plugins.greeter = {
  plugin = greeting.packages.${system}.default;
  settings.greeting = "Hi";
};
```

`Cargo.lock` is intentionally not part of the template. Run
`cargo generate-lockfile` in the development shell before the first `nix build`.
