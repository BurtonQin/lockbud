# Contributing to lockbud

Thanks for your interest in improving lockbud! This document describes how to set
up a development environment, the conventions for commits and pull requests, and
what CI expects before a change can be merged.

## Development setup

lockbud is a rustc plugin: it links against `rustc_private` internals, so it must
be built with the **exact nightly** pinned in [`rust-toolchain.toml`](rust-toolchain.toml).
The pinned version changes as rustc internals evolve — always rely on the file
instead of a remembered version.

```sh
git clone https://github.com/BurtonQin/lockbud.git
cd lockbud
# rustup reads rust-toolchain.toml and installs the pinned nightly + components
rustup component add rustfmt clippy
cargo build
```

Rust-analyzer users: the repo already sets `rustc_private = true` under
`[package.metadata.rust-analyzer]`, so code navigation works out of the box with
the pinned toolchain installed.

### Everyday commands

| Command | Purpose |
| --- | --- |
| `cargo build` | Build the `lockbud` and `cargo-lockbud` binaries |
| `cargo test` | Run the unit tests (regexes, options parsing, graph analyses) |
| `cargo fmt --all -- --check` | Formatting check — CI gate |
| `cargo clippy --workspace --all-targets -- -D warnings` | Lint check — CI gate |
| `./detect.sh toys/inter` | End-to-end smoke test; must report DoubleLock bugs |

See the `toys/` directory for small crates that exercise each detector
(`inter`, `intra`, `panic`, `use-after-free`, `atomic-violation`, ...). When you
change detector behavior, run the relevant toys before and after your change.

## Commit message convention

We follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<optional scope>): <short summary>

<optional body>

<optional footer(s)>
```

- **Summary**: imperative mood, no trailing period, ideally ≤ 72 characters.
- **Scope** (optional): `deadlock`, `atomicity`, `memory`, `panic`, `pointsto`,
  `options`, `toys`, `toolchain`, `docs`, ...
- **Body** (optional): explain *why* the change is needed, not just what it does;
  wrap at ~72 characters. Reference issues with `Fixes #123`.

Allowed types:

| Type | Used for |
| --- | --- |
| `feat` | New detector, pattern, flag, or output capability |
| `fix` | Bug fix: false positive, false negative, crash, build breakage |
| `docs` | Documentation only (README, comments, CONTRIBUTING) |
| `style` | Formatting, no semantic change |
| `refactor` | Internal restructuring without behavior change |
| `perf` | Analysis performance improvement |
| `test` | Unit tests or new/updated `toys/` cases |
| `build` | Cargo dependencies, profiles, build scripts |
| `ci` | GitHub Actions and CI configuration |
| `chore` | Maintenance (toolchain bumps, minor cleanups) |
| `revert` | Reverting a previous commit |

Examples matching this repo's history:

```
feat: disable dependency checking by default; add -r to detect.sh
fix: rustc-hash dep error; closure upvar tracking for issues
chore: bump rustc version to 2026-02-07
```

Tip: run `git config commit.template .gitmessage` once so `git commit` opens an
editor pre-filled with a reminder of this format.

## Pull request guidelines

1. **PR title must follow the same convention as commits** (e.g.
   `fix(deadlock): carry lock guards through Result methods`). A CI check
   (`.github/workflows/pr-title.yml`) rejects non-conforming titles.
2. **Keep PRs focused.** One detector fix or one feature per PR; unrelated
   cleanups belong in separate PRs.
3. **CI must be green.** Required checks include rustfmt, clippy (`-D warnings`),
   unit tests, the `toys/inter` smoke test, and a README/toolchain sync check —
   see `.github/workflows/ci.yml`.
4. **Prove detector changes.** If your PR changes what a detector reports:
   - add or update a case in `toys/`, and
   - include a short before/after summary of the reports in the PR description.
5. **Update the README** when you change usage, output format, or the pinned
   nightly (CI checks that README mentions the toolchain from
   `rust-toolchain.toml`).
6. **Nightly bumps** must update `rust-toolchain.toml` and README together, fix
   any rustc-private API breakage, and re-run the toys to compare reports.

## Reporting issues

Use the issue templates: bug reports should include the lockbud commit, the
rustc nightly you ran it with, the detector kind (`-k`), a minimal reproducer,
and the full output. Feature requests should describe the missed bug pattern or
workflow and, ideally, real-world code that motivates it.

## Releasing (maintainers)

Releases are automated by `.github/workflows/release.yml`:

1. Bump `version` in `Cargo.toml` (and `Cargo.lock` via `cargo build`).
2. Tag: `git tag vX.Y.Z && git push origin vX.Y.Z` — the tag must match the
   Cargo.toml version.
3. The workflow builds release binaries for Linux (x86_64) and macOS (aarch64),
   uploads tarballs to the GitHub Release, and generates release notes.
