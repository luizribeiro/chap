#!/bin/sh

set -eu

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

assert_file() {
  [ -f "$1" ] || fail "expected file: $1"
}

assert_equal() {
  [ "$1" = "$2" ] || fail "expected '$2', got '$1'"
}

plugin_files() {
  printf '%s\n' \
    chap_openai_compatible.wasm \
    chap_kagi.wasm \
    chap_exec_plugin.wasm \
    chap_state_plugin.wasm \
    chap_workspace_plugin.wasm \
    chap_persona.wasm
}

make_artifacts() {
  artifacts=$1

  mkdir -p \
    "$artifacts/plugins" \
    "$artifacts/microsandbox/bin" \
    "$artifacts/microsandbox/lib"
  cat > "$artifacts/chap" <<'EOF'
#!/bin/sh
printf 'MSB_PATH=%s\n' "$MSB_PATH"
printf 'MSB_LIBKRUNFW_PATH=%s\n' "$MSB_LIBKRUNFW_PATH"
config=
previous=
for argument in "$@"; do
  printf '%s\n' "$argument"
  if [ "$previous" = --config ]; then
    config=$argument
  fi
  previous=$argument
done
if [ -n "${TEST_SMOKE_CONFIG_CAPTURE:-}" ] && [ -n "$config" ]; then
  cp "$config" "$TEST_SMOKE_CONFIG_CAPTURE"
fi
if [ "${TEST_FAIL_PLUGIN_CHECK:-0}" = 1 ] &&
   [ "${previous:-}" = check ]; then
  exit 1
fi
EOF
  chmod 755 "$artifacts/chap"

  for plugin in $(plugin_files); do
    : > "$artifacts/plugins/$plugin"
  done

  : > "$artifacts/microsandbox/bin/msb"
  : > "$artifacts/microsandbox/lib/libkrunfw.5.dylib"
  ln -s libkrunfw.5.dylib "$artifacts/microsandbox/lib/libkrunfw.dylib"
  : > "$artifacts/microsandbox/lib/libkrunfw.so.5.6.1"
  ln -s libkrunfw.so.5.6.1 "$artifacts/microsandbox/lib/libkrunfw.so.5"
  ln -s libkrunfw.so.5 "$artifacts/microsandbox/lib/libkrunfw.so"
}

assert_wrapper() {
  root=$1
  libkrunfw_file=$2

  links=$work/links-$(basename "$root")
  mkdir -p "$links"
  ln -s "$root/bin/chap" "$links/chap"
  actual=$(env -u MSB_PATH -u MSB_LIBKRUNFW_PATH "$links/chap" 'alpha beta' --flag)
  expected=$(printf 'MSB_PATH=%s\nMSB_LIBKRUNFW_PATH=%s\nalpha beta\n--flag' \
    "$root/lib/microsandbox/bin/msb" \
    "$root/lib/microsandbox/lib/$libkrunfw_file")
  [ "$actual" = "$expected" ] || fail "wrapper output did not match"

  actual=$(MSB_PATH=custom-msb MSB_LIBKRUNFW_PATH=custom-libkrunfw \
    "$links/chap")
  expected=$(printf 'MSB_PATH=custom-msb\nMSB_LIBKRUNFW_PATH=custom-libkrunfw')
  [ "$actual" = "$expected" ] || fail "wrapper did not preserve overrides"
}

script_dir=$(CDPATH='' cd "$(dirname "$0")" && pwd)
repo=$(CDPATH='' cd "$script_dir/../.." && pwd)
packager=$repo/scripts/package-release.sh
work=$(mktemp -d "${TMPDIR:-/tmp}/chap-package-test.XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM

package_and_check() {
  target=$1
  libkrunfw_file=$2
  versioned_libkrunfw_file=$3

  out=$work/out-$target
  extracted=$work/extracted-$target
  package_name=chap-test-version-$target
  archive=$out/$package_name.tar.gz
  checksum=$archive.sha256

  TEST_SMOKE_CONFIG_CAPTURE=$capture "$packager" \
    --version test-version \
    --target "$target" \
    --out "$out" \
    --artifacts "$artifacts" >/dev/null

  assert_file "$archive"
  assert_file "$checksum"
  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$out" && sha256sum -c "$(basename "$checksum")") >/dev/null
  else
    (cd "$out" && shasum -a 256 -c "$(basename "$checksum")") >/dev/null
  fi

  mkdir -p "$extracted"
  tar -C "$extracted" -xzf "$archive"
  root=$extracted/$package_name
  for path in \
    bin/chap \
    libexec/chap \
    lib/microsandbox/bin/msb \
    "lib/microsandbox/lib/$versioned_libkrunfw_file" \
    share/chap/chap.json.in
  do
    assert_file "$root/$path"
  done
  for plugin in $(plugin_files); do
    assert_file "$root/lib/plugins/$plugin"
  done
  [ -L "$root/lib/microsandbox/lib/$libkrunfw_file" ] ||
    fail "$libkrunfw_file was not preserved as a symlink"

  shellcheck "$root/bin/chap"
  assert_wrapper "$root" "$libkrunfw_file"
}

artifacts=$work/artifacts
capture=$work/smoke.json
make_artifacts "$artifacts"

bundle_mapping() {
  bash -c '
    . "$1"
    select_microsandbox_bundle "$2"
    printf "%s|%s|%s\n" \
      "$microsandbox_bundle_file" \
      "$microsandbox_lib_file" \
      "$microsandbox_lib_alias"
  ' _ "$packager" "$1"
}

assert_equal "$(bundle_mapping aarch64-apple-darwin)" \
  'microsandbox-darwin-aarch64.tar.gz|libkrunfw.5.dylib|libkrunfw.dylib'
assert_equal "$(bundle_mapping x86_64-unknown-linux-gnu)" \
  'microsandbox-linux-x86_64.tar.gz|libkrunfw.so.5.6.1|libkrunfw.so'
assert_equal "$(bundle_mapping aarch64-unknown-linux-gnu)" \
  'microsandbox-linux-aarch64.tar.gz|libkrunfw.so.5.6.1|libkrunfw.so'

if bash -c '. "$1"; select_microsandbox_bundle "$2"' \
  _ "$packager" x86_64-pc-windows-msvc >/dev/null 2>&1
then
  fail "microsandbox bundle selection accepted an unsupported target"
fi

checksum_dir=$work/checksum
archive=$checksum_dir/microsandbox-darwin-aarch64.tar.gz
checksums=$checksum_dir/checksums.sha256
mkdir -p "$checksum_dir"
printf 'runtime bytes\n' > "$archive"
digest=$(bash -c '. "$1"; sha256_digest "$2"' _ "$packager" "$archive")
printf '%s  %s\n' "$digest" "$(basename "$archive")" > "$checksums"
bash -c '. "$1"; verify_microsandbox_checksum "$2" "$3"' \
  _ "$packager" "$archive" "$checksums"

printf '%064d  %s\n' 0 "$(basename "$archive")" > "$checksums"
if bash -c '. "$1"; verify_microsandbox_checksum "$2" "$3"' \
  _ "$packager" "$archive" "$checksums" >/dev/null 2>&1
then
  fail "microsandbox checksum verification accepted a mismatch"
fi

printf '%s  another-file.tar.gz\n' "$digest" > "$checksums"
if bash -c '. "$1"; verify_microsandbox_checksum "$2" "$3"' \
  _ "$packager" "$archive" "$checksums" >/dev/null 2>&1
then
  fail "microsandbox checksum verification accepted a missing entry"
fi

lock_dir=$work/lock
mkdir -p "$lock_dir"
cat > "$lock_dir/Cargo.lock" <<'EOF_LOCK'
[[package]]
name = "microsandbox-network"
version = "9.8.7"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "microsandbox"
version = "9.8.7"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = [
 "microsandbox-utils",
]

[[package]]
name = "microsandbox-utils"
version = "9.8.7"
source = "registry+https://github.com/rust-lang/crates.io-index"
EOF_LOCK
assert_equal "$(bash -c '. "$1"; microsandbox_version "$2"' _ "$packager" "$lock_dir/Cargo.lock")" 9.8.7
if bash -c '. "$1"; microsandbox_version "$2"' _ "$packager" /dev/null | grep -q .; then
  fail "microsandbox_version reported a version for a lockfile without the crate"
fi

locked_version=$(bash -c '. "$1"; microsandbox_version "$2"' _ "$packager" "$repo/Cargo.lock")
release_dir=$work/release/v$locked_version
runtime_src=$work/runtime-src
mkdir -p "$release_dir" "$runtime_src"
printf 'msb bytes\n' > "$runtime_src/msb"
printf 'libkrunfw bytes\n' > "$runtime_src/libkrunfw.5.dylib"
tar -C "$runtime_src" -czf "$release_dir/microsandbox-darwin-aarch64.tar.gz" msb libkrunfw.5.dylib
digest=$(bash -c '. "$1"; sha256_digest "$2"' _ "$packager" "$release_dir/microsandbox-darwin-aarch64.tar.gz")
printf '%s  microsandbox-darwin-aarch64.tar.gz\n' "$digest" > "$release_dir/checksums.sha256"

runtime_dest=$work/runtime-dest
MICROSANDBOX_RELEASE_URL=file://$work/release \
  bash -c '. "$1"; fetch_microsandbox_runtime "$2" "$3"' _ "$packager" "$runtime_dest" aarch64-apple-darwin
assert_file "$runtime_dest/bin/msb"
assert_file "$runtime_dest/lib/libkrunfw.5.dylib"
assert_equal "$(readlink "$runtime_dest/lib/libkrunfw.dylib")" libkrunfw.5.dylib
[ ! -e "$runtime_dest/download" ] || fail "fetch_microsandbox_runtime left its download directory behind"

printf '%064d  microsandbox-darwin-aarch64.tar.gz\n' 0 > "$release_dir/checksums.sha256"
if MICROSANDBOX_RELEASE_URL=file://$work/release \
  bash -c '. "$1"; fetch_microsandbox_runtime "$2" "$3"' _ "$packager" "$work/runtime-dest-bad" aarch64-apple-darwin >/dev/null 2>&1
then
  fail "fetch_microsandbox_runtime installed a bundle whose checksum did not match"
fi

package_and_check aarch64-apple-darwin libkrunfw.dylib libkrunfw.5.dylib
package_and_check x86_64-unknown-linux-gnu libkrunfw.so libkrunfw.so.5.6.1

assert_file "$capture"
if grep -q '@CHAP_HOME@' "$capture"; then
  fail "smoke config still contains @CHAP_HOME@"
fi

if TEST_FAIL_PLUGIN_CHECK=1 "$packager" \
  --version negative \
  --target aarch64-apple-darwin \
  --out "$work/negative-out" \
  --artifacts "$artifacts" >/dev/null 2>&1
then
  fail "packager succeeded when plugins check failed"
fi

if "$packager" \
  --version negative \
  --target x86_64-pc-windows-msvc \
  --out "$work/unsupported-out" \
  --artifacts "$artifacts" >/dev/null 2>&1
then
  fail "packager succeeded for a target without a wrapper library name"
fi

echo "package-release tests passed"
