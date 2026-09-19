# npm distribution for sql-optimizer-cli

Publishes the Rust binary as an npm package (esbuild/turbo-style) so users get
`sql-optimizer-cli` on their PATH with a single command — no Rust toolchain
required:

```bash
npm install -g sql-optimizer-cli
sql-optimizer-cli                      # launches the TUI dashboard
sql-optimizer-cli tui --db sqlite://demo.db
sql-optimizer-cli analyze "SELECT 1" --db sqlite::memory:
```

## Layout

```
npm/
├─ package.json      # wrapper: bin entry + optionalDependencies (platform pkgs)
├─ bin/cli.js        # launcher: resolves the platform binary and spawns it
└─ platform/         # assembled by scripts/release-npm.sh (not committed)
   ├─ sql-optimizer-cli-darwin-arm64/
   ├─ sql-optimizer-cli-darwin-x64/
   ├─ sql-optimizer-cli-linux-x64/
   └─ sql-optimizer-cli-linux-arm64/
```

How it works:

- `optionalDependencies` with per-package `os`/`cpu` fields make npm download
  only the platform package matching the user's machine.
- `bin/cli.js` finds the binary inside that package and spawns it with
  `stdio: 'inherit'`, so the TUI keeps its TTY (raw mode, colors) and exit
  codes pass through unchanged (0/1/2/3).
- Bare invocation defaults to the TUI (`normalize_invocation` in `src/cli/mod.rs`).

## Publishing

### Via CI (recommended)

1. Add an npm automation token as the `NPM_TOKEN` repository secret.
2. Bump `version` in `Cargo.toml` and `npm/package.json` (they should match).
3. Push a tag: `git tag v0.2.0 && git push origin v0.2.0`.
   `.github/workflows/release-npm.yml` builds each target on a matching
   runner and publishes the four platform packages + the wrapper.

### Locally

```bash
scripts/release-npm.sh 0.2.0            # build + assemble under npm/platform/
scripts/release-npm.sh 0.2.0 --publish  # ... and npm publish everything
```

Linux musl targets need `cross` (`cargo install cross`); the darwin targets
build on a Mac directly.

## Notes

- Unscoped package names are used for simplicity; switch to a scope
  (`@yourscope/sql-optimizer-cli`) if the bare names are taken on npm.
- `npm/platform/` is generated — add it to `.gitignore`.
- Windows is not supported (matching the Rust tool itself), enforced by the
  `os` fields in both package.json files.
