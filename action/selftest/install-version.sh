#!/usr/bin/env bash
# Which release does install.sh try to download? A moving tag (v0, v0.1)
# resolves to the action's own version; any other ref is used as given.
# Offline: curl and rustup are stand-ins, so no download or build happens.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd -P)"
t="$(mktemp -d)"
trap 'rm -rf "$t"' EXIT
mkdir -p "$t/bin" "$t/action"
cat > "$t/bin/curl" <<'EOF'
#!/usr/bin/env bash
for a in "$@"; do last="$a"; done
echo "$last" >> "$FAKE_CURL_LOG"
printf 404
EOF
printf '#!/bin/sh\nexit 1\n' > "$t/bin/rustup"
chmod +x "$t/bin/curl" "$t/bin/rustup"

fail=0
# check <own version> <action ref> <release tag expected in the URL, or "">
check() {
  printf '[package]\nname = "gitent"\nversion = "%s"\n' "$1" > "$t/action/Cargo.toml"
  : > "$t/log"
  FAKE_CURL_LOG="$t/log" PATH="$t/bin:$PATH" GITENT_VERSION="" GITENT_ACTION_REF="$2" \
    GITENT_ACTION_PATH="$t/action" RUNNER_TOOL_CACHE="$t/cache" RUNNER_TEMP="$t" \
    bash "$here/install.sh" >/dev/null 2>&1 || true
  got="$(head -1 "$t/log" | sed -n 's|.*/releases/download/\([^/]*\)/.*|\1|p')"
  if [ "$got" = "$3" ]; then
    echo "ok   own=$1 ref='$2' -> '${got}'"
  else
    echo "FAIL own=$1 ref='$2' -> '${got}', expected '$3'"
    fail=1
  fi
}

check 0.1.0  v0      v0.1.0
check 0.1.0  v0.1    v0.1.0
check 0.1.0  v0.1.0  v0.1.0
check 0.10.0 v0.1    v0.1     # v0.1 is not the series of 0.10.0
check 10.0.0 v1      v1       # nor is v1 the series of 10.0.0
check 0.1.0  v1      v1
check 0.1.0  main    main
check 0.1.0  ""      ""       # no ref (uses: ./): build from source
exit "$fail"
