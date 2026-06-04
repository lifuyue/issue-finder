#!/bin/sh
set -eu

repo="${PATCHBAY_REPO:-lifuyue/patchbay-cli}"
version="${PATCHBAY_VERSION:-latest}"
install_dir="${PATCHBAY_INSTALL_DIR:-$HOME/.local/bin}"

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "patchbay installer: missing required command: $1" >&2
    exit 1
  fi
}

need_cmd curl
need_cmd tar

os="$(uname -s)"
arch="$(uname -m)"

case "$os:$arch" in
  Darwin:arm64 | Darwin:aarch64)
    asset="patchbay-aarch64-apple-darwin.tar.gz"
    ;;
  Darwin:x86_64)
    asset="patchbay-x86_64-apple-darwin.tar.gz"
    ;;
  Linux:x86_64 | Linux:amd64)
    asset="patchbay-x86_64-unknown-linux-gnu.tar.gz"
    ;;
  *)
    echo "patchbay installer: unsupported platform: $os $arch" >&2
    echo "Download a release manually from https://github.com/$repo/releases" >&2
    exit 1
    ;;
esac

if [ "$version" = "latest" ]; then
  base_url="https://github.com/$repo/releases/latest/download"
else
  base_url="https://github.com/$repo/releases/download/$version"
fi

tmp_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT INT TERM

archive="$tmp_dir/$asset"
checksums="$tmp_dir/SHA256SUMS"

echo "Downloading Patchbay CLI $version for $os $arch..."
curl -fsSL "$base_url/$asset" -o "$archive"

if command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1; then
  curl -fsSL "$base_url/SHA256SUMS" -o "$checksums"
  checksum_line="$(grep " $asset\$" "$checksums" || true)"
  if [ -z "$checksum_line" ]; then
    echo "patchbay installer: SHA256SUMS does not include $asset" >&2
    exit 1
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    printf '%s\n' "$checksum_line" | (cd "$tmp_dir" && sha256sum -c -)
  else
    printf '%s\n' "$checksum_line" | (cd "$tmp_dir" && shasum -a 256 -c -)
  fi
else
  echo "patchbay installer: sha256sum or shasum not found; skipping checksum verification" >&2
fi

tar -xzf "$archive" -C "$tmp_dir"

if [ ! -f "$tmp_dir/patchbay" ]; then
  echo "patchbay installer: archive did not contain a patchbay executable" >&2
  exit 1
fi

mkdir -p "$install_dir"
cp "$tmp_dir/patchbay" "$install_dir/patchbay"
chmod 755 "$install_dir/patchbay"

echo "Installed patchbay to $install_dir/patchbay"

case ":$PATH:" in
  *":$install_dir:"*) ;;
  *)
    echo "Add $install_dir to your PATH to run patchbay from any shell."
    ;;
esac
