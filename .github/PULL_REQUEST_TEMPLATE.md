<!--
The PR title must follow Conventional Commits, e.g. `fix(deadlock): carry guards through Option::map`.
See CONTRIBUTING.md for the allowed types and details (CI enforces this).
-->

## Summary

<!-- What does this PR change and why? Link related issues, e.g. "Fixes #123". -->

## Changes

<!-- Bullet list of the notable changes. -->

## Testing

<!-- How did you verify the change? e.g. `cargo test`, `./detect.sh toys/<case>` -->

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test` passes
- [ ] Detector behavior verified with `./detect.sh toys/<case>` (or N/A)

## Checklist

- [ ] A `toys/` case is added or updated for detector behavior changes (or N/A)
- [ ] README.md is updated when the nightly, usage, or reported output changes (or N/A)
