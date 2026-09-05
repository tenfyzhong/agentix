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

The test suite is layered. Protocol and rendering tests cover pure mappings; adapter tests cover Telegram, Feishu, Pi, and Codex transports; and core tests exercise routing, persistence, actions, interactions, and lifecycle transitions. Full-stack tests pass mocked Telegram and Feishu events through the channel adapter, engine, and Codex client before verifying the completed response at the channel API.

Codex uses a stateful mock app-server under `crates/agentix-codex/tests/support/`. It follows the Codex CLI 0.153.0 protocol subset used by Agentix, including session lifecycle, settings, approvals, input questions, pagination, failures, and reconnects. Telegram and Feishu use in-process API services, Pi uses a reusable fake RPC subprocess, and rmux tests exchange typed SDK packets with a Unix-socket mock daemon. These fixtures keep the suite deterministic and independent of live credentials, public networks, local session data, and running daemons.

GitHub Actions keeps formatting and Clippy in `ci.yml`. The `tests.yml` workflow runs the full suite on Linux and macOS and checks the workspace plus the native TCP control suite on Windows. It runs for pull requests and pushes to `main`, and supports manual dispatch.

## Workspace architecture

The main crates are `agentix-core`, `agentix-codex`, `agentix-pi`, `agentix-telegram`, `agentix-feishu`, and the `agentix` executable.

The core exposes a small common agent interface plus optional queue, attached-session control, and workspace-runtime ports. A serialized runtime loop feeds IM and agent events into coordinator-owned session, turn, interaction, and rmux state. See the [architecture document](docs/architecture.md) for the state/effect and retry boundaries.

Run `make` for a debug build, `make release` for a release build, or `make help` to list the available targets.

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

The repository keeps `[workspace.package].version` and its workspace entries in `Cargo.lock` at `0.0.0-dev`. Create a semantic-version tag with an optional leading `v`, for example `v0.2.0`. The release workflow derives the release version from that tag and updates the checked-out Cargo metadata before compiling; release versions are not committed to the development branch.

Pushing the tag starts the `Release` workflow, which:

1. verifies that the tag points at the checked-out commit and contains a supported semantic version;
2. applies that version to the workspace manifest and lockfile, then builds native binaries for macOS arm64, Linux x86_64/arm64, and Windows x86_64;
3. verifies each binary's `--version` against the tag;
4. publishes native archives, `SHA256SUMS`, and generated notes to the matching GitHub Release;
5. invokes the Homebrew workflow after the GitHub Release is available.

The Homebrew formula is maintained exclusively in [`tenfyzhong/homebrew-tap`](https://github.com/tenfyzhong/homebrew-tap/blob/main/Formula/agentix.rb); edit dependencies, installation steps, and service settings there. Do not add a formula template to this repository. The formula applies its source tag version to the Cargo metadata before its locked source build. The workflow checks out the tap, updates the existing formula's source URL and checksum, and removes stale bottle metadata while preserving the tap's other settings. It then builds an arm64 macOS bottle, uploads it to the release, adds its metadata, and opens or updates a PR in the tap. Automatic and manually dispatched publishing both require a `HOMEBREW_TAP_TOKEN` with permission to create branches and pull requests.

Before tagging a release:

1. Run formatting, Clippy, all tests, and documentation tests.
2. Run `agentix doctor` as the intended runtime user.
3. Verify the selected channel's owner allowlist and group mention behavior.
4. Exercise concurrent sessions and confirm their cards update independently.
5. Restart Agentix during an active turn and verify that the original message recovers its Stop action and completes in place.
6. Restart the Codex daemon and verify reconnect and subscription recovery.
7. Attach a fresh Codex TUI before its first prompt, send that prompt from IM, and verify that the session materializes and resumes.
