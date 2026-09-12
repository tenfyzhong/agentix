# Repository Guidelines

## Project Structure & Module Organization

Agentix is a Rust 2024 workspace under `crates/`. `agentix` and `taskix` provide the CLIs; `agentix-domain`, `agentix-core`, and `agentix-storage` separate contracts, orchestration, and persistence. Other crates implement agent, messaging, and terminal adapters. Keep unit tests beside source and integration tests in each crate's `tests/` directory. `plugins/` contains host integrations and plugin tests; `docs/` holds architecture and usage guides; `completions/` contains generated shell assets.

## Build, Test, and Development Commands

Use the pinned `rust-toolchain.toml` toolchain and Node.js 24+ with npm.

- `make`: build the workspace with all features in debug mode.
- `make release`: build optimized binaries.
- `make check`: check formatting, run Clippy with warnings denied, and execute Rust and Node plugin tests.
- `make test`: install locked plugin dependencies and run both test suites.
- `cargo fmt --all`: format Rust code; `make fmt` only checks formatting.
- `cargo run -p agentix -- serve`: run locally after configuring Agentix.
- `make completions`: regenerate both CLIs' completions after command changes.

## Coding Style & Naming Conventions

Follow rustfmt, using four-space Rust indentation and two spaces for YAML/JSON. Use `snake_case` for functions/modules, `PascalCase` for types, and `SCREAMING_SNAKE_CASE` for constants. Avoid trailing whitespace. Unsafe Rust is forbidden. Use English for documentation, comments, identifiers, messages, and test labels. Preserve transport boundaries and update related documentation when behavior changes.

## Testing Guidelines

When changes affect only one module, run only that module's tests to shorten feedback time; for one crate, use `cargo test -p <crate> --all-features`. Skip workspace-wide tests for these changes.

Use Rust `#[test]`, asynchronous `#[tokio::test]`, and Node's test runner. Name tests in descriptive `snake_case` that identifies the behavior. For behavior changes, first add a reusable regression test and confirm the expected failure, then implement the minimum fix. Documentation-only changes are exempt. Use deterministic mocks and isolated databases; avoid live credentials or developer sessions. Run focused tests with `cargo test -p agentix-core <test_name>`; see `docs/integration-coverage.md` for coverage boundaries.

## Commit & Pull Request Guidelines

Pull before editing. Create branches such as `docs/example` from current `main`, with matching worktrees under `.git/wtm/docs/example`. Never commit or push to `main`. Use concise English subjects with prefixes such as `feat:`, `fix:`, or `docs:` and sign off every commit using `git commit -s`. PRs should explain the problem, resulting behavior, validation, and compatibility impact. Keep titles/descriptions accurate, synchronize documentation, and exclude secrets and build artifacts. See `CONTRIBUTING.md` for the full workflow.
