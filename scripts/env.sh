#!/usr/bin/env sh
# Source this file to get the $BIN variable and a `sqlopt` shortcut command
# for sql-optimizer-cli, so you never type `cargo run -- ...` or the full
# target/debug path again.
#
# Try it out (current shell only):
#   source scripts/env.sh
#   $BIN schema --db sqlite::memory:
#   sqlopt schema --db sqlite::memory:
#
# Make it permanent (pick your shell):
#   echo 'source "$HOME/path/to/sql-optimizer-cli/scripts/env.sh"' >> ~/.zshrc
#   echo 'source "$HOME/path/to/sql-optimizer-cli/scripts/env.sh"' >> ~/.bashrc
#
# Or install the launcher permanently without touching rc files:
#   scripts/env.sh --install       # copies sqlopt to ~/.local/bin (or ~/bin)
#
# After sourcing:
#   BIN            -> absolute path to the debug binary (built on demand)
#   sqlopt <cmd>   -> runs the binary with any args, e.g.:
#                     sqlopt schema --db sqlite::memory:
#                     sqlopt tui --db sqlite://demo.db
#   sqlopt-rebuild -> cargo build, then re-check

# Resolve repo root relative to this file (portable across machines/clones).
_repo_root=$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")/.." && pwd)
BIN="$_repo_root/target/debug/sql-optimizer-cli"
unset _repo_root

# Build on first use if the binary is missing (only when sourcing interactively).
if [ ! -x "$BIN" ] && [ -t 0 ]; then
    echo "[env.sh] $BIN not found — building (cargo build)..." >&2
    (cd "$(dirname "$BIN")/.." && cargo build) || {
        echo "[env.sh] cargo build failed; fix build errors first." >&2
        return 1 2>/dev/null || exit 1
    }
fi

sqlopt() {
    "$BIN" "$@"
}

sqlopt-rebuild() {
    (cd "$(dirname "$BIN")/.." && cargo build) || return 1
    echo "OK: $BIN is up to date."
}

# --install: make `sqlopt` available in new shells without sourcing this file.
if [ "${1:-}" = "--install" ]; then
    _dest_dir="${HOME}/.local/bin"
    mkdir -p "$_dest_dir"
    cat > "$_dest_dir/sqlopt" <<LAUNCHER
#!/usr/bin/env sh
BIN="$BIN"
if [ ! -x "\$BIN" ]; then
    echo "sqlopt: binary not found at \$BIN" >&2
    echo "        run 'cargo build' in $(dirname "$BIN")/.. first" >&2
    exit 1
fi
exec "\$BIN" "\$@"
LAUNCHER
    chmod +x "$_dest_dir/sqlopt"
    case ":$PATH:" in
        *":$_dest_dir:"*) ;;
        *) echo "NOTE: $_dest_dir is not on your PATH. Add it to your rc file:" >&2
           echo "  echo 'export PATH=\"\$_dest_dir:\$PATH\"' >> ~/.zshrc" >&2 ;;
    esac
    echo "Installed: $_dest_dir/sqlopt"
    echo "Test in a NEW shell:  sqlopt schema --db sqlite::memory:"
    unset _dest_dir
    return 0 2>/dev/null || exit 0
fi

echo "[env.sh] loaded: BIN=$BIN (try: sqlopt schema --db sqlite::memory:)" >&2
