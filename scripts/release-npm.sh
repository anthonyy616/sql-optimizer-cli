#!/usr/bin/env bash
# Build release binaries for every supported target and assemble the npm
# platform packages (esbuild-style layout) under npm/platform/.
#
# Usage:
#   scripts/release-npm.sh [version]            # build + assemble
#   scripts/release-npm.sh [version] --publish  # build + assemble + npm publish
#
# Targets (Unix only — Windows is not supported):
#   aarch64-apple-darwin       (macOS Apple Silicon)
#   x86_64-apple-darwin        (macOS Intel)
#   x86_64-unknown-linux-musl  (Linux x64, static — works on any distro)
#   aarch64-unknown-linux-musl (Linux ARM64, static)
#
# Requirements: rustup (for target_add), cross (for Linux targets), npm.
# CI (.github/workflows/release-npm.yml) runs one target per runner instead
# of building all four on a single machine.

set -euo pipefail

VERSION="${1:-}"
PUBLISH=false
if [ "${2:-}" = "--publish" ]; then
  PUBLISH=true
fi

if [ -z "$VERSION" ]; then
  VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
  echo "No version argument — using Cargo.toml version: $VERSION"
fi

REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$REPO_ROOT"

OUT_DIR="npm/platform"
BIN_NAME="sql-optimizer-cli"

# target-triple | npm-suffix
TARGETS=(
  "aarch64-apple-darwin|darwin-arm64"
  "x86_64-apple-darwin|darwin-x64"
  "x86_64-unknown-linux-musl|linux-x64"
  "aarch64-unknown-linux-musl|linux-arm64"
)

for entry in "${TARGETS[@]}"; do
  target="${entry%%|*}"
  suffix="${entry##*|}"
  pkg_name="${BIN_NAME}-${suffix}"
  pkg_dir="${OUT_DIR}/${pkg_name}"

  echo "==> Building $target"
  rustup target add "$target" 2>/dev/null || true
  if [[ "$target" == *musl* ]]; then
    # musl targets need cross (musl-gcc + perl for vendored OpenSSL)
    if ! command -v cross >/dev/null 2>&1; then
      echo "musl target needs 'cross' — install with: cargo install cross --locked" >&2
      exit 1
    fi
    cross build --release --target "$target"
  else
    # Fall back to plain cargo when building the host's own darwin target.
    cargo build --release --target "$target" 2>/dev/null || cargo build --release
  fi
  BUILT="target/${target}/release/${BIN_NAME}"
  [ -x "$BUILT" ] || BUILT="target/release/${BIN_NAME}"

  echo "==> Assembling $pkg_name"
  rm -rf "$pkg_dir"
  mkdir -p "$pkg_dir"
  cp "$BUILT" "$pkg_dir/$BIN_NAME"
  chmod +x "$pkg_dir/$BIN_NAME"

  case "$suffix" in
    darwin-arm64) os=darwin; cpu=arm64 ;;
    darwin-x64)   os=darwin; cpu=x64 ;;
    linux-x64)    os=linux;  cpu=x64 ;;
    linux-arm64)  os=linux;  cpu=arm64 ;;
  esac

  cat > "$pkg_dir/package.json" <<EOF
{
  "name": "$pkg_name",
  "version": "$VERSION",
  "description": "Prebuilt sql-optimizer-cli binary for ${os}-${cpu}",
  "license": "MIT",
  "repository": {
    "type": "git",
    "url": "https://github.com/anthonyy616/sql-optimizer-cli.git"
  },
  "os": ["$os"],
  "cpu": ["$cpu"],
  "files": ["$BIN_NAME"]
}
EOF

  if $PUBLISH; then
    echo "==> Publishing $pkg_name@$VERSION"
    (cd "$pkg_dir" && npm publish --access public)
  fi
done

# Sanity check the wrapper resolves on this machine.
node -e "require.resolve('$(pwd)/npm/bin/cli.js')" >/dev/null

echo
echo "Assembled packages in $OUT_DIR/:"
ls -1 "$OUT_DIR"
if $PUBLISH; then
  echo "==> Publishing wrapper package"
  (cd npm && npm publish --access public)
else
  echo "Dry run only. To publish everything:"
  echo "  for d in $OUT_DIR/*/; do (cd \"\$d\" && npm publish --access public); done"
  echo "  (cd npm && npm publish --access public)"
fi
