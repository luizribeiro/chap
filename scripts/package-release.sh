#!/usr/bin/env bash

set -euo pipefail

usage() {
  echo "usage: $0 [--version V] [--target TRIPLE] [--out DIR] [--artifacts DIR]" >&2
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

default_version() {
  git describe --tags --always --dirty | sed 's/^v//'
}

default_target() {
  rustc -vV | sed -n 's/^host: //p'
}

microsandbox_version() {
  local lockfile=$1

  awk '
    $0 == "[[package]]" { package_name = "" }
    $1 == "name" && $3 == "\"microsandbox\"" { package_name = "microsandbox"; next }
    package_name == "microsandbox" && $1 == "version" {
      gsub(/"/, "", $3)
      print $3
      exit
    }
  ' "$lockfile"
}

select_microsandbox_bundle() {
  case $1 in
    aarch64-apple-darwin)
      microsandbox_bundle_file=microsandbox-darwin-aarch64.tar.gz
      microsandbox_lib_file=libkrunfw.5.dylib
      microsandbox_lib_alias=libkrunfw.dylib
      ;;
    x86_64-unknown-linux-gnu)
      microsandbox_bundle_file=microsandbox-linux-x86_64.tar.gz
      microsandbox_lib_file=libkrunfw.so.5.6.1
      microsandbox_lib_alias=libkrunfw.so
      ;;
    aarch64-unknown-linux-gnu)
      microsandbox_bundle_file=microsandbox-linux-aarch64.tar.gz
      microsandbox_lib_file=libkrunfw.so.5.6.1
      microsandbox_lib_alias=libkrunfw.so
      ;;
    *)
      echo "error: unsupported microsandbox target: $1" >&2
      return 1
      ;;
  esac
}

sha256_digest() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{ print $1 }'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{ print $1 }'
  else
    echo "error: sha256sum or shasum is required" >&2
    return 1
  fi
}

verify_microsandbox_checksum() {
  local archive=$1
  local checksums=$2
  local archive_name
  local expected
  local actual

  archive_name=$(basename "$archive")
  if ! expected=$(awk -v archive="$archive_name" '$2 == archive { print $1; found = 1; exit } END { if (!found) exit 1 }' "$checksums"); then
    echo "error: checksum not found for $archive_name" >&2
    return 1
  fi
  actual=$(sha256_digest "$archive")
  if [ "$actual" != "$expected" ]; then
    echo "error: checksum mismatch for $archive_name" >&2
    return 1
  fi
}

fetch_microsandbox_runtime() {
  local destination=$1
  local target=$2
  local version
  local release_url
  local download_dir=$destination/download
  local archive
  local checksums=$download_dir/checksums.sha256

  select_microsandbox_bundle "$target"
  version=$(microsandbox_version "$repo/Cargo.lock")
  if [ -z "$version" ]; then
    echo "error: microsandbox version not found in Cargo.lock" >&2
    return 1
  fi
  release_url=${MICROSANDBOX_RELEASE_URL:-https://github.com/superradcompany/microsandbox/releases/download}/v$version
  archive=$download_dir/$microsandbox_bundle_file

  mkdir -p "$destination/bin" "$destination/lib" "$download_dir"
  curl -fsSL --output "$checksums" "$release_url/checksums.sha256"
  curl -fsSL --output "$archive" "$release_url/$microsandbox_bundle_file"
  verify_microsandbox_checksum "$archive" "$checksums"
  tar -C "$download_dir" -xzf "$archive" msb "$microsandbox_lib_file"
  mv "$download_dir/msb" "$destination/bin/msb"
  mv "$download_dir/$microsandbox_lib_file" "$destination/lib/$microsandbox_lib_file"
  ln -s "$microsandbox_lib_file" "$destination/lib/$microsandbox_lib_alias"
  rm -rf "$download_dir"
}

build() {
  local artifacts=$1
  local target=$2
  local plugin

  mkdir -p "$artifacts/plugins"
  (
    cd "$repo"
    cargo build --release --locked --target wasm32-wasip2 \
      -p chap-openai-compatible \
      -p chap-kagi \
      -p chap-exec-plugin \
      -p chap-state-plugin \
      -p chap-workspace-plugin \
      -p chap-persona
  )

  for plugin in $(plugin_files); do
    cp "$repo/target/wasm32-wasip2/release/$plugin" "$artifacts/plugins/$plugin"
  done

  (
    cd "$repo"
    cargo build --release --locked -p chap-cli --features vm --target "$target"
  )
  cp "$repo/target/$target/release/chap" "$artifacts/chap"
  fetch_microsandbox_runtime "$artifacts/microsandbox" "$target"
}

write_wrapper() {
  local destination=$1
  local target=$2
  local libkrunfw_file

  case $target in
    *-darwin) libkrunfw_file=libkrunfw.dylib ;;
    *-linux-*) libkrunfw_file=libkrunfw.so ;;
    *) echo "error: unsupported package target: $target" >&2; return 1 ;;
  esac

  cat > "$destination" <<EOF
#!/bin/sh

set -eu

libkrunfw_file=$libkrunfw_file
EOF
  cat >> "$destination" <<'EOF'

script=$0
while [ -L "$script" ]; do
  script_dir=$(CDPATH='' cd "$(dirname "$script")" && pwd)
  link=$(readlink "$script")
  case $link in
    /*) script=$link ;;
    *) script=$script_dir/$link ;;
  esac
done
script_dir=$(CDPATH='' cd "$(dirname "$script")" && pwd)
root=$(CDPATH='' cd "$script_dir/.." && pwd)

MSB_PATH=${MSB_PATH-"$root/lib/microsandbox/bin/msb"}
MSB_LIBKRUNFW_PATH=${MSB_LIBKRUNFW_PATH-"$root/lib/microsandbox/lib/$libkrunfw_file"}
export MSB_PATH MSB_LIBKRUNFW_PATH
exec "$root/libexec/chap" "$@"
EOF
  chmod 755 "$destination"
}

write_config_template() {
  local destination=$1

  cat > "$destination" <<'EOF'
{
  "name": "chap",
  "agent": {
    "vm": { "registries": ["docker.io"] },
    "workspace": {
      "image": "docker.io/library/alpine:3.20",
      "mount": "rw",
      "egress": ["0.0.0.0/0:443", "0.0.0.0/0:80", "0.0.0.0/0:53"]
    }
  },
  "plugins": {
    "openai": {
      "component": "@CHAP_HOME@/lib/plugins/chap_openai_compatible.wasm",
      "settings": {
        "base_url": "http://127.0.0.1:8080/v1",
        "model": "example-model",
        "api_key_env": "OPENAI_API_KEY"
      }
    },
    "workspace": {
      "component": "@CHAP_HOME@/lib/plugins/chap_workspace_plugin.wasm"
    },
    "persona": {
      "component": "@CHAP_HOME@/lib/plugins/chap_persona.wasm",
      "settings": { "persona": "Be concise and practical.", "priority": -10 }
    }
  }
}
EOF
}

check_relocatable() {
  local binary=$1
  local dependencies

  case $(uname -s) in
    Darwin) dependencies=$(otool -L "$binary" 2>&1 || true) ;;
    Linux) dependencies=$(ldd "$binary" 2>&1 || true) ;;
    *) dependencies="dynamic dependency inspection is unsupported on $(uname -s)" ;;
  esac
  printf '%s\n' "$dependencies" >&2

  if printf '%s\n' "$dependencies" | grep -q '/nix/store'; then
    if [ -n "${CI:-}" ]; then
      echo "error: $binary has a dynamic dependency in /nix/store" >&2
      return 1
    fi
    echo "warning: $binary has a dynamic dependency in /nix/store" >&2
  fi
}

render_smoke_config() {
  local template=$1
  local root=$2
  local destination=$3
  local escaped_root

  escaped_root=$(printf '%s' "$root" | sed 's/[&|\\]/\\&/g')
  sed "s|@CHAP_HOME@|$escaped_root|g" "$template" > "$destination"
}

write_checksum() {
  local archive=$1
  local archive_dir
  local archive_name

  archive_dir=$(dirname "$archive")
  archive_name=$(basename "$archive")
  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$archive_dir" && sha256sum "$archive_name") > "$archive.sha256"
  elif command -v shasum >/dev/null 2>&1; then
    (cd "$archive_dir" && shasum -a 256 "$archive_name") > "$archive.sha256"
  else
    echo "error: sha256sum or shasum is required" >&2
    return 1
  fi
}

assemble() {
  local artifacts=$1
  local staging=$2
  local package_name=$3
  local output_dir=$4
  local target=$5
  local tree=$staging/$package_name
  local smoke=$staging/smoke
  local archive=$output_dir/$package_name.tar.gz
  local plugin

  mkdir -p \
    "$tree/bin" \
    "$tree/libexec" \
    "$tree/lib/microsandbox/bin" \
    "$tree/lib/microsandbox/lib" \
    "$tree/lib/plugins" \
    "$tree/share/chap" \
    "$smoke"

  cp "$artifacts/chap" "$tree/libexec/chap"
  chmod 755 "$tree/libexec/chap"
  check_relocatable "$tree/libexec/chap"
  cp "$artifacts/microsandbox/bin/msb" "$tree/lib/microsandbox/bin/msb"
  cp -a "$artifacts/microsandbox/lib/." "$tree/lib/microsandbox/lib/"

  for plugin in $(plugin_files); do
    cp "$artifacts/plugins/$plugin" "$tree/lib/plugins/$plugin"
  done

  write_wrapper "$tree/bin/chap" "$target"
  write_config_template "$tree/share/chap/chap.json.in"
  render_smoke_config \
    "$tree/share/chap/chap.json.in" \
    "$tree" \
    "$smoke/chap.json"
  "$tree/bin/chap" \
    --config "$smoke/chap.json" plugins check

  tar -C "$staging" -czf "$archive" "$package_name"
  write_checksum "$archive"
  printf '%s\n%s\n' "$archive" "$archive.sha256"
}

script_dir=$(CDPATH='' cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(CDPATH='' cd "$script_dir/.." && pwd)

main() {
  local version=
  local target=
  local out=dist
  local artifacts=

  while [ "$#" -gt 0 ]; do
    case $1 in
      --version|--target|--out|--artifacts)
        if [ "$#" -lt 2 ]; then
          usage
          exit 2
        fi
        case $1 in
          --version) version=$2 ;;
          --target) target=$2 ;;
          --out) out=$2 ;;
          --artifacts) artifacts=$2 ;;
        esac
        shift 2
        ;;
      -h|--help)
        usage
        exit 0
        ;;
      *)
        usage
        exit 2
        ;;
    esac
  done

  version=${version:-$(default_version)}
  target=${target:-$(default_target)}

  case $version in
    ''|*/*) echo "error: invalid version: $version" >&2; exit 2 ;;
  esac
  case $target in
    ''|*/*) echo "error: invalid target: $target" >&2; exit 2 ;;
  esac
  mkdir -p "$out"
  out=$(CDPATH='' cd "$out" && pwd)
  package_work=$(mktemp -d "${TMPDIR:-/tmp}/chap-package.XXXXXX")
  trap 'rm -rf "$package_work"' EXIT HUP INT TERM

  if [ -n "$artifacts" ]; then
    artifacts=$(CDPATH='' cd "$artifacts" && pwd)
  else
    artifacts=$package_work/artifacts
    build "$artifacts" "$target"
  fi

  assemble "$artifacts" "$package_work" "chap-$version-$target" "$out" "$target"
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  main "$@"
fi
