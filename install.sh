#!/bin/sh
# Install the gitent release binary (Linux and macOS), checked against the
# release's published .sha256.
#
#   curl -fsSL https://raw.githubusercontent.com/eladb3/gitent/main/install.sh | sh
#
# Environment:
#   GITENT_VERSION      release tag to install (default: the latest release)
#   GITENT_INSTALL_DIR  where to put the binary (default: ~/.local/bin)
set -eu

repo="eladb3/gitent"
dir="${GITENT_INSTALL_DIR:-$HOME/.local/bin}"
version="${GITENT_VERSION:-}"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)               target="x86_64-unknown-linux-gnu" ;;
  Linux-aarch64|Linux-arm64)  target="aarch64-unknown-linux-gnu" ;;
  Darwin-arm64)               target="aarch64-apple-darwin" ;;
  Darwin-x86_64)              target="x86_64-apple-darwin" ;;
  *) echo "gitent: no prebuilt binary for $(uname -s) $(uname -m); build it with: cargo install gitent" >&2
     exit 1 ;;
esac

if [ -n "$version" ]; then
  base="https://github.com/$repo/releases/download/$version"
else
  base="https://github.com/$repo/releases/latest/download"
fi
asset="gitent-$target.tar.gz"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/gitent-install.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT INT TERM

echo "gitent: downloading $base/$asset"
curl -fsSL --retry 3 -o "$tmp/$asset" "$base/$asset"
curl -fsSL --retry 3 -o "$tmp/$asset.sha256" "$base/$asset.sha256"

expected="$(awk 'NR==1 {print tolower($1)}' "$tmp/$asset.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/$asset" | awk '{print $1}')"
else
  actual="$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')"
fi
if ! printf '%s' "$expected" | grep -Eq '^[0-9a-f]{64}$' || [ "$expected" != "$actual" ]; then
  echo "gitent: checksum mismatch for $asset (expected '$expected', got '$actual'); nothing installed" >&2
  exit 1
fi

tar -xzf "$tmp/$asset" -C "$tmp" gitent
mkdir -p "$dir"
cp "$tmp/gitent" "$dir/gitent.new"
chmod 755 "$dir/gitent.new"
mv -f "$dir/gitent.new" "$dir/gitent"
echo "gitent: installed $("$dir/gitent" --version) to $dir/gitent (sha256 $actual)"

case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "gitent: $dir is not on your PATH; add it, e.g. export PATH=\"$dir:\$PATH\"" ;;
esac
