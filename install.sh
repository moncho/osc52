#!/bin/sh
# Install osc52 from its GitHub releases:
#
#   curl -fsSL https://raw.githubusercontent.com/moncho/osc52/main/install.sh | sh
#
# OSC52_INSTALL_DIR  where to install (default: ~/.local/bin)
# OSC52_VERSION      release tag to install, such as v0.3.0 (default: latest)
set -eu

main() {
  dir=${OSC52_INSTALL_DIR:-$HOME/.local/bin}
  version=${OSC52_VERSION:-latest}

  case $(uname -s) in
    Linux) os=unknown-linux-musl ;;
    Darwin) os=apple-darwin ;;
    *) die "unsupported system: $(uname -s)" ;;
  esac
  case $(uname -m) in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) die "unsupported architecture: $(uname -m)" ;;
  esac
  asset=osc52-$arch-$os

  if [ "$version" = latest ]; then
    base=https://github.com/moncho/osc52/releases/latest/download
  else
    base=https://github.com/moncho/osc52/releases/download/$version
  fi

  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  download "$base/$asset" "$tmp/$asset"
  download "$base/$asset.sha256" "$tmp/$asset.sha256"
  (cd "$tmp" && verify "$asset.sha256") || die "checksum mismatch for $asset"

  mkdir -p "$dir"
  chmod 755 "$tmp/$asset"
  mv "$tmp/$asset" "$dir/osc52"
  echo "installed $("$dir/osc52" --version) to $dir/osc52"

  case :$PATH: in
    *:"$dir":*) ;;
    *) echo "add $dir to your PATH to run it as osc52" ;;
  esac
}

download() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$2" "$1" || die "download failed: $1"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$2" "$1" || die "download failed: $1"
  else
    die "curl or wget is required"
  fi
}

verify() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c "$1" >/dev/null 2>&1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -c "$1" >/dev/null 2>&1
  else
    die "sha256sum or shasum is required"
  fi
}

die() {
  echo "osc52 install: $*" >&2
  exit 1
}

main "$@"
