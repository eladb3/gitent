#!/usr/bin/env bash
# Install gitent: release binary for the requested version (checked against the
# release's published .sha256), else build the action's own source.
set -euo pipefail
dest="${RUNNER_TOOL_CACHE:-$HOME/.cache}/gitent/bin"
mkdir -p "$dest"
version="${GITENT_VERSION:-}"
[ -z "$version" ] && version="${GITENT_ACTION_REF:-}"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)  target="x86_64-unknown-linux-gnu" ;;
  Linux-aarch64) target="aarch64-unknown-linux-gnu" ;;
  Darwin-arm64)  target="aarch64-apple-darwin" ;;
  Darwin-x86_64) target="x86_64-apple-darwin" ;;
  *) target="" ;;
esac

sha256_of() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | awk '{print $1}'
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}

# Download $1 to $2; prints the HTTP status (000 on network errors).
fetch() {
  curl -sSL --retry 3 -o "$2" -w '%{http_code}' "$1" 2>/dev/null || true
}

installed=""
if [ -n "$version" ] && [ "$version" != "source" ] && [ -n "$target" ]; then
  tmp="$(mktemp -d "${RUNNER_TEMP:-/tmp}/gitent-dl.XXXXXX")"
  asset="gitent-${target}.tar.gz"
  url="https://github.com/eladb3/gitent/releases/download/${version}/${asset}"
  echo "gitent: trying release binary $url"
  if [ "$(fetch "$url" "$tmp/$asset")" = "200" ]; then
    status="$(fetch "$url.sha256" "$tmp/$asset.sha256")"
    if [ "$status" = "200" ]; then
      expected="$(awk 'NR==1 {print tolower($1)}' "$tmp/$asset.sha256")"
      actual="$(sha256_of "$tmp/$asset")"
      if ! printf '%s' "$expected" | grep -Eq '^[0-9a-f]{64}$' || [ "$expected" != "$actual" ]; then
        # Fail closed: a published checksum that does not match is never ignored.
        echo "::error::gitent: checksum mismatch for $asset (expected '${expected}', got '${actual}'); refusing to install"
        exit 1
      fi
      echo "gitent: sha256 verified ($actual)"
      tar -xzf "$tmp/$asset" -C "$dest" gitent
      installed="release $version"
    else
      echo "::warning::gitent: release $version has no checksum asset ($asset.sha256: HTTP $status); building from source instead"
    fi
  fi
  rm -rf "$tmp"
fi

if [ -z "$installed" ]; then
  echo "gitent: building from source (${GITENT_ACTION_PATH})"
  command -v cargo >/dev/null || { curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal; . "$HOME/.cargo/env"; }
  cargo build --release --locked --manifest-path "${GITENT_ACTION_PATH}/Cargo.toml" --target-dir "${RUNNER_TEMP:-/tmp}/gitent-target"
  cp "${RUNNER_TEMP:-/tmp}/gitent-target/release/gitent" "$dest/"
  installed="source"
fi
chmod +x "$dest/gitent"
echo "$dest" >> "$GITHUB_PATH"
"$dest/gitent" --version
echo "gitent: installed ($installed)"
