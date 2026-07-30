# Repo & tooling conventions — sql-optimizer-cli

Concrete, low-level facts about how this specific repo is worked on. These aren't preferences,
they're things that have already caused wasted time when ignored.

## Environment

- Development happens in **WSL on Windows**. Treat the working environment as Linux for all
  practical purposes (paths, shell, cargo) — do not suggest native Windows tooling or PowerShell.
- The shipped product is **Unix-only (Linux/macOS)**. Never add Windows-specific code paths,
  Windows packaging, or Windows CI targets. If a dependency or crate feature only matters for
  Windows support, skip it.
- Repo: `anthonyy616/sql-optimizer-cli` on GitHub. Active development happens on a **test branch**,
  not directly on `main` — check current branch before committing and confirm the intended target
  branch if it's ambiguous.

## Reading the repo

- **`git clone` via bash is reliable. `web_fetch` against GitHub URLs is not** — don't try to read
  repo files by fetching GitHub URLs; clone the repo (or pull if already cloned) and read from
  disk instead.
- Some files in this repo are **UTF-16LE encoded** (a known housekeeping bug tracked in Phase 0 —
  historically this has hit `Makefile`, files under `docs/`, and files under `tests/`). Before
  assuming a file is empty or garbled, check its encoding:
  ```bash
  file path/to/file
  ```
  If it reports UTF-16, convert before reading:
  ```bash
  iconv -f UTF-16LE -t UTF-8 path/to/file
  ```
  Do not "fix" this by rewriting content from scratch without first checking whether real content
  was already there in a different encoding — that's how real work gets silently discarded.
- To get a full, trustworthy file tree (this has been more reliable than editor/IDE file panes or
  partial `ls`), use:
  ```bash
  find . -type f -not -path './.git/*' | sort
  ```

## Cargo / build conventions

- `pip`-equivalent note doesn't apply here (this is a pure Rust project), but when any Python
  tooling is used alongside it (e.g. for scripting checks), remember this container's pip requires
  `--break-system-packages`.
- The binary name in `Cargo.toml` is `sql-optimizer-cli`. A known historical bug had
  `install.sh`/`Makefile` copying to a binary literally named `sql-optimizer` — always verify the
  `[[bin]] name` in `Cargo.toml` matches whatever `make install`/`scripts/install.sh` actually
  copies, every time either file is touched.
- `serde_json` has, at least once, already been present as a Cargo feature/dependency when an
  agent's progress report described it as missing or unfinished. **Check `Cargo.toml` directly
  before adding or "fixing" a dependency that a progress report claims is absent.**

## Design rules that constrain every change (see implementation-plan.md §3 for full list)

These are easy to violate accidentally while implementing something else. Check against them
before considering a change done:

- **One engine, two consumption modes.** Manual and CI/pipeline usage must be differentiated by
  CLI arguments (e.g. `--ci`), never by a separate code path or separate binary.
- **`analyze` and `batch` must never prompt for input, confirmation, or "did you mean...?"** —
  that behavior belongs only to `interactive`. A prompt that's harmless for a human hangs a CI job
  until timeout. Grep for any interactive-input calls (`dialoguer::Input`, etc.) creeping into
  `analyze`/`batch`/`scan` code paths.
- **Every recommendation carries a confidence label**: syntactic guess / schema-verified /
  plan-verified / orm-heuristic (added in Phase 6). Don't add a new recommendation type without
  deciding which tier it belongs to.
- **The `profile` parameter (`oltp` | `analytics`) is threaded through the whole pipeline**, even
  in phases before it's actively used. New detector functions should accept it in their signature
  from the start rather than needing a second pass later.
- **The tool is stateless by default.** Any feature that persists data across runs (regression
  history, tracked baselines) must be opt-in (presence of `.sql-optimizer/` or an explicit flag
  like `--track`) — a plain `analyze` call must never write to disk unannounced.
- **Supabase and Neon are not new connectors** — they're the existing Postgres connector handling
  real-world deployment quirks (pooler modes, TLS, cold starts). Don't scaffold a separate
  `supabase.rs`/`neon.rs` connector.
- **Convex is out of scope.** Don't add scaffolding for it without an explicit, separate decision
  to do so first.

## Anthony's working style

- During planning/design discussion, the preferred mode is **conceptual review and critique, not
  unsolicited code changes**. When asked to review a plan, an agent's progress report, or an
  architectural choice, give a direct assessment — don't jump straight to patching code unless
  asked to implement.
- When implementation is explicitly requested, proceed normally and build the actual thing.