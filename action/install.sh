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
  # Build the action's OWN checkout, isolated from the PR (ADR 0016 section 2):
  # cargo reads .cargo/config.toml from the current directory and its parents,
  # rustup reads rust-toolchain(.toml) from the current directory, and both
  # read environment variables an earlier step may have set via $GITHUB_ENV.
  # So: cwd is a fresh directory outside the workspace, CARGO_HOME is fresh,
  # the toolchain is named explicitly, the environment is minimal, --locked.
  src="${GITENT_ACTION_PATH:?GITENT_ACTION_PATH is not set}"
  src="$(cd "$src" && pwd -P)"
  ws="$(cd "${GITHUB_WORKSPACE:-/nonexistent}" 2>/dev/null && pwd -P || true)"
  if [ -n "$ws" ]; then
    case "$src/" in
      "$ws"/*) echo "::warning::gitent: the action's source is inside the workspace ($src); building it builds the checked-out code (fine for 'uses: ./' self-tests only)" ;;
    esac
  fi
  build="$(mktemp -d "${RUNNER_TEMP:-/tmp}/gitent-build.XXXXXX")"
  mkdir -p "$build/cargo-home" "$build/tmp"
  toolchain="stable"
  # Only the action's own pin counts, never the PR's.
  if [ -f "$src/rust-toolchain" ]; then
    toolchain="$(head -1 "$src/rust-toolchain" | tr -d '[:space:]')"
  fi
  rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"
  rustup_bin="$(command -v rustup || true)"
  if [ -z "$rustup_bin" ]; then
    curl -fsSL https://sh.rustup.rs | env -i PATH=/usr/bin:/bin HOME="$HOME" RUSTUP_HOME="$rustup_home" CARGO_HOME="$build/rustup-bin" \
      sh -s -- -y --profile minimal --no-modify-path --default-toolchain none
    rustup_bin="$build/rustup-bin/bin/rustup"
  fi
  # rustup itself runs with the CARGO_HOME it is installed in; cargo gets a fresh one.
  rustup_cargo_home="$(dirname "$(dirname "$rustup_bin")")"
  net_env=()
  # Network settings for runners behind a proxy (crates are checksum-verified by --locked).
  for v in HTTPS_PROXY https_proxy HTTP_PROXY http_proxy NO_PROXY no_proxy SSL_CERT_FILE CARGO_HTTP_CAINFO; do
    if [ -n "${!v:-}" ]; then net_env+=("$v=${!v}"); fi
  done
  rustup_env=(env -i "PATH=/usr/local/bin:/usr/bin:/bin" "HOME=$HOME" "RUSTUP_HOME=$rustup_home"
              "CARGO_HOME=$rustup_cargo_home" "${net_env[@]}")
  echo "gitent: building from source ($src, toolchain $toolchain, in $build)"
  (
    cd "$build"
    "${rustup_env[@]}" "$rustup_bin" toolchain install "$toolchain" --profile minimal
    # The toolchain's own binaries, not rustup's proxies (which would consult
    # rust-toolchain files and RUSTUP_TOOLCHAIN).
    tc_bin="$(dirname "$("${rustup_env[@]}" "$rustup_bin" which --toolchain "$toolchain" cargo)")"
    env -i "PATH=$tc_bin:/usr/local/bin:/usr/bin:/bin" "HOME=$HOME" "TMPDIR=$build/tmp" \
      "CARGO_HOME=$build/cargo-home" "RUSTC=$tc_bin/rustc" "${net_env[@]}" \
      "$tc_bin/cargo" build --release --locked \
      --manifest-path "$src/Cargo.toml" --target-dir "$build/target"
  )
  cp "$build/target/release/gitent" "$dest/"
  installed="source ($toolchain)"
fi
chmod +x "$dest/gitent"
echo "$dest" >> "$GITHUB_PATH"
"$dest/gitent" --version
echo "gitent: installed ($installed)"
