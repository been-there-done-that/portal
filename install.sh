#!/usr/bin/env bash
set -euo pipefail

REPO="deesh/portal"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
BINARY="portal"

detect_platform() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os" in
    Darwin)
      case "$arch" in
        arm64) echo "aarch64-apple-darwin" ;;
        x86_64) echo "x86_64-apple-darwin" ;;
        *) echo "unsupported arch: $arch" >&2; exit 1 ;;
      esac ;;
    Linux)
      case "$arch" in
        aarch64) echo "aarch64-unknown-linux-gnu" ;;
        x86_64) echo "x86_64-unknown-linux-gnu" ;;
        *) echo "unsupported arch: $arch" >&2; exit 1 ;;
      esac ;;
    *) echo "unsupported OS: $os" >&2; exit 1 ;;
  esac
}

latest_version() {
  curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' \
    | sed 's/.*"tag_name": *"\(.*\)".*/\1/'
}

main() {
  local platform version url archive
  platform="$(detect_platform)"
  version="${VERSION:-$(latest_version)}"
  archive="portal-${version}-${platform}.tar.gz"
  url="https://github.com/${REPO}/releases/download/${version}/${archive}"

  echo "Installing portal ${version} for ${platform}..."
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT

  curl -fsSL "$url" -o "$tmp/$archive"
  tar -xzf "$tmp/$archive" -C "$tmp"
  install -m 755 "$tmp/portal" "$INSTALL_DIR/$BINARY"

  echo "portal installed to $INSTALL_DIR/$BINARY"
  echo "Run: portal --help"
}

main "$@"
