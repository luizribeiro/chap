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
printf 'MSB_HOME=%s\n' "$MSB_HOME"
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
}

script_dir=$(CDPATH='' cd "$(dirname "$0")" && pwd)
repo=$(CDPATH='' cd "$script_dir/../.." && pwd)
packager=$repo/scripts/package-release.sh
work=$(mktemp -d "${TMPDIR:-/tmp}/chap-package-test.XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM

artifacts=$work/artifacts
out=$work/out
extracted=$work/extracted
capture=$work/smoke.json
package_name=chap-test-version-test-target
archive=$out/$package_name.tar.gz
checksum=$archive.sha256
make_artifacts "$artifacts"

TEST_SMOKE_CONFIG_CAPTURE=$capture "$packager" \
  --version test-version \
  --target test-target \
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
  lib/microsandbox/lib/libkrunfw.5.dylib \
  share/chap/chap.json.in
do
  assert_file "$root/$path"
done
for plugin in $(plugin_files); do
  assert_file "$root/lib/plugins/$plugin"
done
[ -L "$root/lib/microsandbox/lib/libkrunfw.dylib" ] ||
  fail "libkrunfw.dylib was not preserved as a symlink"

links=$work/links
mkdir -p "$links"
ln -s "$root/bin/chap" "$links/chap"
state=$work/state
msb_home=$state/chap/microsandbox
actual=$(XDG_STATE_HOME=$state "$links/chap" 'alpha beta' --flag)
expected=$(printf 'MSB_HOME=%s\nalpha beta\n--flag' "$msb_home")
[ "$actual" = "$expected" ] || fail "wrapper output did not match"
[ "$(readlink "$msb_home/bin/msb")" = "$root/lib/microsandbox/bin/msb" ] ||
  fail "state directory does not link to the packaged msb"
for library in libkrunfw.5.dylib libkrunfw.dylib; do
  [ "$(readlink "$msb_home/lib/$library")" = "$root/lib/microsandbox/lib/$library" ] ||
    fail "state directory does not link to the packaged $library"
done

assert_file "$capture"
if grep -q '@CHAP_HOME@' "$capture"; then
  fail "smoke config still contains @CHAP_HOME@"
fi

if TEST_FAIL_PLUGIN_CHECK=1 "$packager" \
  --version negative \
  --target test-target \
  --out "$work/negative-out" \
  --artifacts "$artifacts" >/dev/null 2>&1
then
  fail "packager succeeded when plugins check failed"
fi

echo "package-release tests passed"
