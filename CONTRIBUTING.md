# Contributing to Agentix

Thank you for helping improve Agentix. Contributions may include code, tests, documentation, bug reports, and design feedback.

## Development environment

Agentix is a Rust workspace. Install the toolchain pinned in `rust-toolchain.toml`; it includes Rust 1.95, rustfmt, and Clippy. Linux CI also installs `protobuf-compiler`.

Clone the repository and verify the workspace before making changes:

```sh
git clone https://github.com/tenfyzhong/agentix.git
cd agentix
make check
```

Live Telegram, Feishu, Codex, Pi, or rmux credentials and services are not required for the normal test suite. Integration tests use local mock services and fake transports.

## Branches and worktrees

Do not commit directly to `main` or push changes to it. Start from the latest `main`, create a clearly named branch, and give that branch a dedicated worktree under `.git/wtm/`:

```sh
git switch main
git pull --ff-only
git worktree add -b feat/example .git/wtm/feat/example main
cd .git/wtm/feat/example
```

Use a prefix that describes the contribution, such as `feat/`, `fix/`, `docs/`, `refactor/`, or `test/`.

## Development workflow

Agentix uses test-driven development for features, bug fixes, refactors, and other behavior changes:

1. Add or update a reusable test that describes the intended behavior.
2. Run it and confirm that it fails for the expected reason.
3. Implement the smallest production change that makes it pass.
4. Run the focused test while iterating.
5. Run the complete quality gate before committing.

Documentation-only and configuration-only changes do not require a failing test first. Keep `README.md` and the documents under `docs/` synchronized with user-visible behavior and architecture changes.

## Tests and external dependencies

Place tests near the boundary they exercise:

- Pure parsing, rendering, and protocol mappings belong in focused unit or adapter tests.
- Routing, persistence, lifecycle, command, approval, and interaction behavior belongs in `agentix-core` orchestration tests.
- Codex protocol sequences belong in the stateful mock app-server suite under `crates/agentix-codex/tests/`.
- Telegram and Feishu transport behavior belongs in their adapter integration suites and should use the in-process Bot API or OpenAPI/WebSocket mocks.
- Pi and Oh My Pi subprocess behavior should use reusable fake RPC processes.

Do not make automated tests depend on live credentials, public network services, a developer's session data, or an already running daemon. Extend the relevant mock when a new external API method, event, error, or state transition is introduced. Keep mock payloads aligned with the upstream wire format used by Agentix.

Run the complete quality gate with:

```sh
make check
```

This is equivalent to:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

## Code and documentation style

- Follow rustfmt and the workspace Clippy configuration.
- Treat warnings as errors.
- Do not add unsafe Rust; the workspace forbids it.
- Keep abstractions at transport boundaries so orchestration can be tested independently.
- Preserve existing user changes and avoid unrelated rewrites.
- Use consistent indentation and leave no trailing whitespace.
- Write code comments and repository documentation in English.

## Commits

Write concise English commit subjects that describe the outcome. Conventional prefixes such as `feat:`, `fix:`, `test:`, `refactor:`, and `docs:` are preferred.

Every commit must include a Developer Certificate of Origin sign-off:

```sh
git commit -s -m "test: cover callback delivery"
```

By adding the sign-off, you certify that you have the right to submit the contribution under the project's license.

## Pull requests

Open a pull request when the branch is ready. Use English for its title and description, and keep both synchronized as the change evolves. A useful description explains:

- the problem and intended behavior;
- the chosen design and important tradeoffs;
- the tests added or updated;
- any operational, compatibility, or migration impact.

Before requesting review, confirm that:

- the change is scoped and contains no unrelated edits;
- behavior changes have regression tests;
- external integrations use deterministic mocks where practical;
- `README.md` and other relevant documentation are current;
- `make check` passes;
- every commit is signed off;
- no secrets, credentials, local session data, or generated build artifacts are included.

Review feedback should normally be addressed with additional commits. Keep the pull request title and description accurate after substantial revisions.

## Releases

Before tagging a release, update `[workspace.package].version` in `Cargo.toml` and refresh `Cargo.lock`. Create a tag with the same version and an optional leading `v`, for example `v0.2.0`; the release workflow rejects mismatches instead of rewriting package metadata during publishing.

The `Release` workflow publishes native archives. Its Homebrew call is currently commented out; the standalone Homebrew workflow remains available for manual publishing once the tap and `HOMEBREW_TAP_TOKEN` are configured. See `docs/development-and-operations.md` for the complete artifact flow and the change required to enable automatic Homebrew publishing later.
