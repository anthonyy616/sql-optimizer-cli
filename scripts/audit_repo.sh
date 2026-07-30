#!/usr/bin/env bash
# audit_repo.sh — deterministic ground-truth checks for sql-optimizer-cli.
#
# Automates steps 1-3 of references/audit-checklist.md: real file tree, stub
# detection, and module wiring checks. Does NOT replace reading the flagged
# files yourself — it surfaces suspects.
#
# Usage:
#   ./audit_repo.sh <path-to-repo-or-git-url> [branch]
#
# If given a URL, clones fresh into a temp dir. If given a local path, uses it
# as-is (does not pull/fetch — do that yourself first if you need latest).

set -euo pipefail

TARGET="${1:?Usage: audit_repo.sh <path-to-repo-or-git-url> [branch]}"
BRANCH="${2:-}"

if [[ "$TARGET" =~ ^https?:// || "$TARGET" =~ ^git@ ]]; then
  WORKDIR="$(mktemp -d)"
  echo "== Cloning $TARGET into $WORKDIR =="
  if [[ -n "$BRANCH" ]]; then
    git clone --depth 1 --branch "$BRANCH" "$TARGET" "$WORKDIR/repo"
  else
    git clone --depth 1 "$TARGET" "$WORKDIR/repo"
  fi
  REPO="$WORKDIR/repo"
else
  REPO="$TARGET"
fi

cd "$REPO"

echo
echo "=================================================================="
echo " 1. FULL FILE TREE (ground truth — compare against any progress report)"
echo "=================================================================="
find . -type f -not -path './.git/*' | sort

echo
echo "=================================================================="
echo " 2. ENCODING CHECK (flags UTF-16 files that may look empty/garbled)"
echo "=================================================================="
find . -type f -not -path './.git/*' \( -name '*.rs' -o -name '*.md' -o -name '*.toml' \
  -o -name 'Makefile' -o -name '*.sh' \) -print0 | while IFS= read -r -d '' f; do
  enc="$(file -b --mime-encoding "$f" 2>/dev/null || echo unknown)"
  if [[ "$enc" == "utf-16le" || "$enc" == "utf-16be" || "$enc" == "utf-16" ]]; then
    echo "UTF-16 DETECTED: $f (encoding: $enc) — convert with: iconv -f UTF-16LE -t UTF-8 '$f'"
  fi
done
echo "(no output above this line = no UTF-16 files found)"

echo
echo "=================================================================="
echo " 3. STUB / PLACEHOLDER DETECTION"
echo "=================================================================="
echo "Files containing ONLY a placeholder marker (likely unimplemented stubs):"
find . -type f -name '*.rs' -not -path './.git/*' -print0 | while IFS= read -r -d '' f; do
  # Strip whitespace/blank lines/comment-only content and see if anything real remains.
  # (grep intentionally returns "no match" here when a file has no real content, so
  # avoid letting that trip pipefail/set -e and silently abort the whole loop.)
  nonstub_lines=$(grep -vE '^\s*(//.*)?$' "$f" 2>/dev/null | grep -vc '// Placeholder implementation' || true)
  if [[ "${nonstub_lines:-0}" -eq 0 ]]; then
    echo "  STUB: $f"
  fi
done

echo
echo "Empty or near-empty test files (0-1 non-blank lines):"
find . -path '*/tests/*' -name '*.rs' -not -path './.git/*' -print0 2>/dev/null | while IFS= read -r -d '' f; do
  nonblank=$(grep -cvE '^\s*$' "$f" 2>/dev/null || true)
  if [[ "${nonblank:-0}" -le 1 ]]; then
    echo "  EMPTY-ISH TEST FILE: $f ($nonblank non-blank lines)"
  fi
done

echo
echo "=================================================================="
echo " 4. MODULE WIRING CHECK (does patterns/security logic have real callers?)"
echo "=================================================================="
for mod in patterns security rewriting; do
  if [[ -d "src/$mod" ]]; then
    echo "-- src/$mod --"
    hits=$(grep -rn "${mod}::" src/ --include='*.rs' | grep -v "src/${mod}/" || true)
    if [[ -z "$hits" ]]; then
      echo "  NOT WIRED UP: no call sites for '${mod}::' outside src/${mod}/ itself."
      echo "  (module is declared but nothing calls into it — check if the equivalent"
      echo "   logic lives inline elsewhere instead, e.g. in core/analyzer.rs)"
    else
      echo "$hits" | sed 's/^/  /'
    fi
  fi
done

echo
echo "=================================================================="
echo " 5. SUSPICIOUS NO-OP PATTERNS (functions that look like no-ops)"
echo "=================================================================="
echo "Grepping for common no-op giveaways — read each hit, don't trust the name alone:"
grep -rn -E "fn with_[a-z_]+\(&self\) -> Self \{\s*$" src/ --include='*.rs' -A1 2>/dev/null | \
  grep -B1 "Self::new()" || echo "  (none found matching this specific pattern)"
echo
grep -rln "unimplemented!\|todo!()" src/ --include='*.rs' 2>/dev/null || echo "  (no unimplemented!()/todo!() found)"

echo
echo "=================================================================="
echo " 6. CARGO.TOML / BINARY NAME SANITY"
echo "=================================================================="
if [[ -f Cargo.toml ]]; then
  echo "Declared [[bin]] name(s):"
  awk '/\[\[bin\]\]/{f=1} f&&/name/{print "  "$0; f=0}' Cargo.toml
  echo
  echo "install.sh / Makefile binary references (should match the name above):"
  grep -n "sql-optimizer" install.sh scripts/install.sh Makefile 2>/dev/null | grep -v "sql-optimizer-cli" || \
    echo "  (no mismatched binary name references found)"
else
  echo "  No Cargo.toml found at repo root."
fi

echo
echo "=================================================================="
echo " Audit scan complete. This is a triage pass, not a verdict — read the"
echo " files flagged above yourself before drawing conclusions."
echo "=================================================================="