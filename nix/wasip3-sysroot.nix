{ pkgs, system }:

let
  sdkAsset = {
    aarch64-darwin = {
      name = "arm64-macos";
      hash = "sha256-gdgfr7zTw7XRqwNVu1e5bv+yzggaJKJ2P3wVUBgJsm4=";
    };
    aarch64-linux = {
      name = "arm64-linux";
      hash = "sha256-hb3N+itPqOH98Z1eDT/tAe/W6BgdSibhat7z/6+KjeY=";
    };
    x86_64-linux = {
      name = "x86_64-linux";
      hash = "sha256-EU7l/mPLXypsD+vO5EYGAmcpgVikCcerOsDgbm1L2c4=";
    };
  }.${system} or null;

  sdkArchive =
    if sdkAsset == null then
      null
    else
      pkgs.fetchurl {
        url = "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-34-rc.2/wasi-sdk-34.0-rc.2-${sdkAsset.name}.tar.gz";
        inherit (sdkAsset) hash;
      };

  sdk =
    if sdkArchive == null then
      null
    else
      pkgs.stdenvNoCC.mkDerivation {
        pname = "wasi-sdk";
        version = "34.0-rc.2";
        src = sdkArchive;
        nativeBuildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
          pkgs.autoPatchelfHook
        ];
        buildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
          pkgs.libxml2
          pkgs.ncurses
          pkgs.stdenv.cc.cc.lib
          pkgs.zlib
          pkgs.zstd
        ];
        installPhase = ''
          runHook preInstall
          cp -R . "$out"
          runHook postInstall
        '';
      };

  libcSource = pkgs.fetchFromGitHub {
    owner = "WebAssembly";
    repo = "wasi-libc";
    rev = "f3e872871c6fb77db9727a18ab812a1a8f6e85ce";
    hash = "sha256-zume/L6ovStZnbfSRX/J0T1XofjyIQGsnp60RpEVabk=";
  };
in
if sdk == null then
  null
else
  pkgs.stdenvNoCC.mkDerivation {
    pname = "wasi-sysroot-wasip3-coop";
    version = "34.0-rc.2";
    src = libcSource;
    nativeBuildInputs = with pkgs; [
      cmake
      gnumake
      wasm-tools
    ];
    cmakeFlags = [
      "-DCMAKE_C_COMPILER=${sdk}/bin/clang"
      "-DCMAKE_AR=${sdk}/bin/llvm-ar"
      "-DCMAKE_RANLIB=${sdk}/bin/llvm-ranlib"
      "-DCMAKE_NM=${sdk}/bin/llvm-nm"
      "-DTARGET_TRIPLE=wasm32-wasip3"
      "-DENABLE_COOP_THREADS=ON"
      "-DBUILD_SHARED=OFF"
      "-DBINDINGS_TARGET=OFF"
      "-DUSE_WASM_COMPONENT_LD=OFF"
      "-DBUILTINS_LIB=${sdk}/lib/clang/23/lib/wasm32-unknown-wasip3/libclang_rt.builtins.a"
    ];
    postInstall = ''
      wrapper_dir=$(mktemp -d)
      cd "$wrapper_dir"
      ${sdk}/bin/llvm-ar x \
        "$out/lib/wasm32-wasip3/libc.a" \
        __cabi_realloc_wrapper.S.obj
      mv __cabi_realloc_wrapper.S.obj \
        "$out/lib/wasm32-wasip3/__cabi_realloc_wrapper.o"
    '';
    passthru = { inherit sdk; };
  }
