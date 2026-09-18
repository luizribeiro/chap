#!/bin/sh

set -eu

repository=luizribeiro/chap
latest_url="https://api.github.com/repos/$repository/releases/latest"
tmp_dir=
stage_root=
config_tmp=

cleanup() {
    if [ -n "$config_tmp" ]; then
        rm -f "$config_tmp"
    fi
    if [ -n "$stage_root" ]; then
        rm -rf "$stage_root"
    fi
    if [ -n "$tmp_dir" ]; then
        rm -rf "$tmp_dir"
    fi
}

fail() {
    printf 'chap installer: %s\n' "$*" >&2
    exit 1
}

supported_targets='aarch64-apple-darwin x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu'

target_supported() {
    for candidate in $supported_targets; do
        [ "$1" = "$candidate" ] && return 0
    done
    return 1
}

detect_target() {
    if [ "${CHAP_TARGET+x}" = x ]; then
        printf '%s\n' "$CHAP_TARGET"
        return
    fi

    system=$(uname -s)
    machine=$(uname -m)
    case "$system:$machine" in
        Darwin:arm64)
            printf '%s\n' aarch64-apple-darwin
            ;;
        Linux:x86_64)
            printf '%s\n' x86_64-unknown-linux-gnu
            ;;
        Linux:aarch64)
            printf '%s\n' aarch64-unknown-linux-gnu
            ;;
        *)
            printf '%s:%s\n' "$system" "$machine"
            ;;
    esac
}

absolute_directory() {
    requested=$1
    case "$requested" in
        /*) absolute=$requested ;;
        *) absolute=$(pwd -P)/$requested ;;
    esac

    parent=$(dirname "$absolute")
    name=$(basename "$absolute")
    mkdir -p "$parent"
    printf '%s/%s\n' "$parent" "$name"
}

verify_checksum() {
    archive=$1
    checksum=$2
    checksum_dir=$(dirname "$checksum")
    checksum_file=$(basename "$checksum")

    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$checksum_dir" && sha256sum -c "$checksum_file")
    elif command -v shasum >/dev/null 2>&1; then
        (cd "$checksum_dir" && shasum -a 256 -c "$checksum_file")
    else
        fail "sha256sum or shasum is required to verify $archive"
    fi
}

trap cleanup EXIT
trap 'exit 1' HUP INT TERM

target=$(detect_target)
target_supported "$target" || fail "unsupported platform '$target'; supported platforms: $supported_targets"

install_dir=$(absolute_directory "${CHAP_INSTALL_DIR:-$HOME/.local/share/chap}")
bin_dir=$(absolute_directory "${CHAP_BIN_DIR:-$HOME/.local/bin}")
[ "$install_dir" != / ] || fail 'CHAP_INSTALL_DIR must not be /'

tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/chap-install.XXXXXX")

if [ "${CHAP_TARBALL+x}" = x ]; then
    [ -n "$CHAP_TARBALL" ] || fail 'CHAP_TARBALL must not be empty'
    tarball=$CHAP_TARBALL
    [ -f "$tarball" ] || fail "tarball not found: $tarball"
    checksum=$tarball.sha256
    version=${CHAP_VERSION:-local}
    version=${version#v}
    if [ -f "$checksum" ]; then
        verify_checksum "$tarball" "$checksum"
    fi
else
    if [ "${CHAP_VERSION+x}" = x ]; then
        version=${CHAP_VERSION#v}
        [ -n "$version" ] || fail 'CHAP_VERSION must not be empty'
    else
        release_json=$(curl -fsSL "$latest_url")
        tag=$(printf '%s\n' "$release_json" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
        case "$tag" in
            v?*) version=${tag#v} ;;
            *) fail 'could not determine the latest release version' ;;
        esac
    fi

    artifact="chap-$version-$target.tar.gz"
    release_url="https://github.com/$repository/releases/download/v$version"
    tarball=$tmp_dir/$artifact
    checksum=$tarball.sha256
    curl -fsSL "$release_url/$artifact" -o "$tarball"
    curl -fsSL "$release_url/$artifact.sha256" -o "$checksum"
    verify_checksum "$tarball" "$checksum"
fi

install_parent=$(dirname "$install_dir")
stage_root=$(mktemp -d "$install_parent/.chap-install.XXXXXX")
staged_install=$stage_root/root
mkdir -p "$staged_install"
tar -xzf "$tarball" -C "$staged_install" --strip-components=1
[ -f "$staged_install/bin/chap" ] || fail 'release archive does not contain bin/chap'
[ -f "$staged_install/share/chap/chap.json.in" ] || fail 'release archive does not contain share/chap/chap.json.in'

rm -rf "$install_dir"
mv "$staged_install" "$install_dir"

mkdir -p "$bin_dir"
link=$bin_dir/chap
if [ -e "$link" ] || [ -L "$link" ]; then
    rm -f "$link"
fi
ln -s "$install_dir/bin/chap" "$link"

config_dir=${XDG_CONFIG_HOME:-$HOME/.config}/chap
config_file=$config_dir/chap.json
if [ ! -e "$config_file" ]; then
    mkdir -p "$config_dir"
    escaped_install_dir=$(printf '%s\n' "$install_dir" | sed 's/[\\&|]/\\&/g')
    config_tmp=$config_file.tmp.$$
    sed "s|@CHAP_HOME@|$escaped_install_dir|g" \
        "$install_dir/share/chap/chap.json.in" > "$config_tmp"
    mv "$config_tmp" "$config_file"
    config_tmp=
fi

printf 'Installed chap %s for %s in %s\n' "$version" "$target" "$install_dir"
case ":${PATH:-}:" in
    *":$bin_dir:"*) ;;
    *) printf 'Add %s to your PATH.\n' "$bin_dir" ;;
esac
printf 'Edit %s and set base_url, model, and api_key_env, then export that API key.\n' "$config_file"
printf 'The first chap run lists plugins to approve with: chap grants review <plugin> && chap grants approve <plugin>\n'
# A sandbox socket path adds 52 characters to chap's state directory and must
# fit in the 104-byte unix socket path limit on macOS.
state_dir=${XDG_STATE_HOME:-$HOME/.local/state}/chap/msb
if [ ${#state_dir} -gt 51 ]; then
    printf "chap's sandbox state directory %s is longer than 51 characters; set XDG_STATE_HOME to a shorter directory before running chap, or its sandbox sockets cannot be created.\n" "$state_dir"
fi
