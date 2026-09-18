#!/bin/sh

set -eu

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

assert_file() {
  [ -f "$1" ] || fail "expected file: $1"
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
