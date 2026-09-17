# Installation

This guide covers supported systems, installer options, download verification, and removal.

## Supported platforms

- Apple Silicon macOS (`aarch64-apple-darwin`)
- Linux on x86-64 (`x86_64-unknown-linux-gnu`) or ARM64 (`aarch64-unknown-linux-gnu`), with `/dev/kvm` available on bare metal or through nested virtualization and the `libcap-ng` shared library installed (`libcap-ng0` on Debian/Ubuntu or `libcap-ng` on Fedora)

The sandbox runtime keeps its sockets under `${XDG_STATE_HOME:-$HOME/.local/state}/chap/msb`, and unix socket paths are capped at 104 bytes on macOS, so that directory must be at most 51 characters long. If your home directory is long, export `MSB_HOME` pointing at a shorter directory before running `chap`; the wrapper refuses to start otherwise and says so.

## Installer options

The installer accepts these optional environment variables:

- `CHAP_VERSION`: release version, with or without the leading `v`; defaults to the latest release
- `CHAP_INSTALL_DIR`: installation root; defaults to `$HOME/.local/share/chap`
- `CHAP_BIN_DIR`: directory for the `chap` symlink; defaults to `$HOME/.local/bin`
- `CHAP_TARBALL`: local release tarball to install instead of downloading one; its `.sha256` sibling is checked when present
- `CHAP_TARGET`: target override, normally detected from the operating system and architecture

For example, to install a specific version:

```sh
curl -fsSL https://raw.githubusercontent.com/luizribeiro/chap/main/install.sh | CHAP_VERSION=1.2.3 sh
```

## Verify a download

Download the release tarball and its matching `.sha256` file from this repository's [Releases page](https://github.com/luizribeiro/chap/releases), then run one of:

```sh
sha256sum -c chap-X.Y.Z-<target>.tar.gz.sha256
shasum -a 256 -c chap-X.Y.Z-<target>.tar.gz.sha256
```

## Uninstall

With the default paths, remove the installation and symlink with:

```sh
rm -rf "$HOME/.local/share/chap"
rm -f "$HOME/.local/bin/chap"
```

Optionally remove the configuration too:

```sh
rm -rf "${XDG_CONFIG_HOME:-$HOME/.config}/chap"
```

Optionally remove the state directory too; it holds plugin approvals, cached VM images and sandboxes:

```sh
rm -rf "${XDG_STATE_HOME:-$HOME/.local/state}/chap"
```
