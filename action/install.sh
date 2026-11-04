#!/usr/bin/env bash
# Install gitent: release binary for the requested version, else build the action's own source.
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

installed=""
if [ -n "$version" ] && [ "$version" != "source" ] && [ -n "$target" ]; then
  url="https://github.com/eladb3/gitent/releases/download/${version}/gitent-${target}.tar.gz"
  echo "gitent: trying release binary $url"
  if curl -fsSL "$url" -o /tmp/gitent.tgz 2>/dev/null; then
    tar -xzf /tmp/gitent.tgz -C "$dest" && installed="release $version"
  fi
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
