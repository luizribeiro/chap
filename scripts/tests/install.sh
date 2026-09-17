#!/bin/sh

set -eu

script_dir=$(CDPATH='' cd "$(dirname "$0")" && pwd)
project_dir=$(CDPATH='' cd "$script_dir/../.." && pwd)
installer=$project_dir/install.sh
test_dir=$(mktemp -d "${TMPDIR:-/tmp}/chap-install-test.XXXXXX")
trap 'rm -rf "$test_dir"' EXIT
trap 'exit 1' HUP INT TERM

fail() {
    printf 'test failed: %s\n' "$*" >&2
    exit 1
}

write_checksum() {
    checksum_archive=$1
    checksum_dir=$(dirname "$checksum_archive")
    checksum_name=$(basename "$checksum_archive")
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$checksum_dir" && sha256sum "$checksum_name" > "$checksum_name.sha256")
    else
        (cd "$checksum_dir" && shasum -a 256 "$checksum_name" > "$checksum_name.sha256")
    fi
}

run_installer() {
    env \
        HOME="$home_dir" \
        XDG_CONFIG_HOME="$config_home" \
        CHAP_TARBALL="$tarball" \
        CHAP_TARGET=x86_64-unknown-linux-gnu \
        CHAP_INSTALL_DIR="$install_dir" \
        CHAP_BIN_DIR="$bin_dir" \
        sh "$installer"
}

package_name=chap-1.2.3-x86_64-unknown-linux-gnu
package_dir=$test_dir/payload/$package_name
mkdir -p "$package_dir/bin" "$package_dir/libexec" \
    "$package_dir/lib/plugins" "$package_dir/share/chap"
printf '%s\n' '#!/bin/sh' 'printf '\''chap stub: %s\n'\'' "$*"' > "$package_dir/bin/chap"
chmod +x "$package_dir/bin/chap"
: > "$package_dir/libexec/chap"
: > "$package_dir/lib/plugins/x.wasm"
printf '{"chap_home":"@CHAP_HOME@"}\n' > "$package_dir/share/chap/chap.json.in"

tarball=$test_dir/$package_name.tar.gz
tar -czf "$tarball" -C "$test_dir/payload" "$package_name"
write_checksum "$tarball"

home_dir=$test_dir/home
config_home=$test_dir/config
install_dir=$test_dir/install/chap
bin_dir=$test_dir/bin
run_installer > "$test_dir/install.out"

[ -f "$install_dir/bin/chap" ] || fail 'installed bin/chap is missing'
[ -L "$bin_dir/chap" ] || fail 'bin directory does not contain the chap symlink'
[ "$("$bin_dir/chap" one two)" = 'chap stub: one two' ] || fail 'installed symlink did not run the wrapper'

config_file=$config_home/chap/chap.json
[ -f "$config_file" ] || fail 'rendered config is missing'
grep -F "$install_dir" "$config_file" >/dev/null || fail 'config does not contain the absolute install directory'
if grep -F '@CHAP_HOME@' "$config_file" >/dev/null; then
    fail 'config still contains @CHAP_HOME@'
fi

printf '{"keep":"this file"}\n' > "$config_file"
cp "$config_file" "$test_dir/config.expected"
run_installer > "$test_dir/reinstall.out"
cmp "$test_dir/config.expected" "$config_file" || fail 'reinstall changed an existing config'

bad_tarball=$test_dir/tampered.tar.gz
cp "$tarball" "$bad_tarball"
printf '%064d  %s\n' 0 "$(basename "$bad_tarball")" > "$bad_tarball.sha256"
bad_install=$test_dir/bad-install
if env \
    HOME="$home_dir" \
    XDG_CONFIG_HOME="$config_home" \
    CHAP_TARBALL="$bad_tarball" \
    CHAP_TARGET=x86_64-unknown-linux-gnu \
    CHAP_INSTALL_DIR="$bad_install" \
    CHAP_BIN_DIR="$test_dir/bad-bin" \
    sh "$installer" > "$test_dir/tampered.out" 2>&1; then
    fail 'installer accepted a tampered checksum'
fi
[ ! -e "$bad_install" ] || fail 'checksum failure left an install directory behind'

if env \
    HOME="$home_dir" \
    XDG_CONFIG_HOME="$config_home" \
    CHAP_TARBALL="$tarball" \
    CHAP_TARGET=unsupported-target \
    CHAP_INSTALL_DIR="$test_dir/unsupported-install" \
    CHAP_BIN_DIR="$test_dir/unsupported-bin" \
    sh "$installer" > "$test_dir/unsupported.out" 2>&1; then
    fail 'installer accepted an unsupported target'
fi
grep -F aarch64-apple-darwin "$test_dir/unsupported.out" >/dev/null || fail 'unsupported message omits macOS target'
grep -F x86_64-unknown-linux-gnu "$test_dir/unsupported.out" >/dev/null || fail 'unsupported message omits Linux target'

plain_dir=$test_dir/plain
mkdir -p "$plain_dir"
plain_tarball=$plain_dir/$(basename "$tarball")
cp "$tarball" "$plain_tarball"
plain_install=$test_dir/plain-install
env \
    HOME="$home_dir" \
    XDG_CONFIG_HOME="$config_home" \
    CHAP_TARBALL="$plain_tarball" \
    CHAP_TARGET=x86_64-unknown-linux-gnu \
    CHAP_INSTALL_DIR="$plain_install" \
    CHAP_BIN_DIR="$test_dir/plain-bin" \
    CHAP_VERSION=v1.2.3 \
    sh "$installer" > "$test_dir/plain.out" 2>&1 || fail 'installer rejected a tarball without a .sha256 sibling'
[ -f "$plain_install/bin/chap" ] || fail 'tarball without a .sha256 sibling was not installed'
grep -F 'Installed chap 1.2.3 for' "$test_dir/plain.out" >/dev/null || fail 'leading v was not stripped from CHAP_VERSION'

arm_install=$test_dir/arm-install
env \
    HOME="$home_dir" \
    XDG_CONFIG_HOME="$config_home" \
    CHAP_TARBALL="$tarball" \
    CHAP_TARGET=aarch64-unknown-linux-gnu \
    CHAP_INSTALL_DIR="$arm_install" \
    CHAP_BIN_DIR="$test_dir/arm-bin" \
    sh "$installer" > "$test_dir/arm.out" 2>&1 || fail 'installer rejected aarch64-unknown-linux-gnu'
[ -f "$arm_install/bin/chap" ] || fail 'aarch64-unknown-linux-gnu install is missing bin/chap'
grep -F 'for aarch64-unknown-linux-gnu in' "$test_dir/arm.out" >/dev/null || fail 'installer did not report the aarch64 target'

short_home=$(mktemp -d /tmp/h.XXXXXX)
trap 'rm -rf "$test_dir" "$short_home"' EXIT
env \
    HOME="$short_home" \
    XDG_CONFIG_HOME="$config_home" \
    CHAP_TARBALL="$tarball" \
    CHAP_TARGET=x86_64-unknown-linux-gnu \
    CHAP_INSTALL_DIR="$test_dir/short-install" \
    CHAP_BIN_DIR="$test_dir/short-bin" \
    sh "$installer" > "$test_dir/short.out" 2>&1 || fail 'installer failed with a short home'
if grep -F 'export MSB_HOME' "$test_dir/short.out" >/dev/null; then
    fail 'installer warned about a short home directory'
fi
long_home=$test_dir/$(printf 'home%.0s' 1 2 3 4 5 6 7 8 9 10)
mkdir -p "$long_home"
env \
    HOME="$long_home" \
    XDG_CONFIG_HOME="$config_home" \
    CHAP_TARBALL="$tarball" \
    CHAP_TARGET=x86_64-unknown-linux-gnu \
    CHAP_INSTALL_DIR="$test_dir/long-install" \
    CHAP_BIN_DIR="$test_dir/long-bin" \
    sh "$installer" > "$test_dir/long.out" 2>&1 || fail 'installer failed with a long home'
grep -F 'export MSB_HOME' "$test_dir/long.out" >/dev/null || fail 'installer did not warn about a long home directory'

printf 'installer tests passed\n'
