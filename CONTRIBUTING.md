# Contributing to Agentix

Thank you for helping improve Agentix. Contributions may include code, tests, documentation, bug reports, and design feedback.

## Development environment

Agentix is a Rust workspace. Install the toolchain pinned in `rust-toolchain.toml`; it includes Rust 1.95, rustfmt, and Clippy. Node.js 24+ and npm run the task plugin tests, including its TypeScript entrypoints against Cargo's freshly compiled `taskix`. Linux CI also installs `protobuf-compiler`.

CI uses Rust 1.95.0. Ensure `cargo`, `rustc`, `cargo-clippy`, and `rustfmt` all come from that toolchain rather than mixing Homebrew and rustup installations.

On macOS and Linux, Python 3.11+ runs the standalone Taskix backup tests included
in `make test`. Run them separately with `make test-backup`; they use a fake
rclone executable and require no cloud credentials or installed rclone.

Clone the repository and verify the workspace before making changes:

```sh
git clone https://github.com/tenfyzhong/agentix.git
cd agentix
make check
```

Live Telegram, Feishu, Codex, Pi, or rmux credentials and services are not required for the normal test suite. Integration tests use local mock services and fake transports; task tests additionally run real CLI subprocesses with isolated databases and document directories.

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

After changing CLI commands or options, run `make completions` and commit the updated files for both CLIs. Tests verify that the checked-in completions match their CLI and that taskix generation does not read configuration or create task state. Checked-in shell completions retain LF line endings on every platform through `.gitattributes`.

## Installing local binaries through Homebrew

Update `tenfyzhong/tap` to a version with precompiled local installation support,
then build and install the current checkout:

```sh
make update VERSION=local

make update VERSION=local PROFILE=debug

make update VERSION=local FORMULAE=taskix
```

`update VERSION=local` runs `make release` for the default release profile or
`make build` for `PROFILE=debug` before installing binaries from
`target/<profile>/`. Both build targets compile the workspace with all features.
Invalid local options or a failed build stop before invoking Homebrew. Missing
or nonexecutable selected binaries after a successful build also stop installation.
`FORMULAE` selects the CLIs to install; the build still covers the workspace.
Rerun the update target after changing source to build and install those changes.

The target passes the current checkout as `HOMEBREW_AGENTIX_LOCAL_SOURCE` and the
build directory as `HOMEBREW_AGENTIX_LOCAL_TARGET_DIR` to
`brew install --build-from-source --skip-link`. The tap snapshots each selected binary,
example configuration and shell completions, then copies them into its keg.
`--build-from-source` selects the local Formula recipe rather than an upstream
bottle; that recipe does not compile or install Rust/LLVM or Protobuf build
dependencies. Stable/remote HEAD builds retain their existing build dependencies.
Configuration, completion and service installation rules remain in the tap.

`PROFILE` defaults to `release` and accepts `release` or `debug`.
`FORMULAE` defaults to `agentix taskix`. For a custom build directory, set
`CARGO_TARGET_DIR` for the build and installation, including paths with spaces:

```sh
make update VERSION=local CARGO_TARGET_DIR=/absolute/path/to/build
```

You can also stage binaries directly with Homebrew, then select the installed
local kegs from the Agentix checkout:

```sh
env HOMEBREW_AGENTIX_LOCAL_SOURCE=/absolute/path/to/agentix \
  brew install --build-from-source --skip-link tenfyzhong/tap/agentix tenfyzhong/tap/taskix
make -C /absolute/path/to/agentix switch VERSION=local
```

Set `HOMEBREW_AGENTIX_LOCAL_PROFILE=debug` or
`HOMEBREW_AGENTIX_LOCAL_TARGET_DIR=/absolute/path/to/build` as needed. Relative
target paths are resolved against the checkout. For an entirely Homebrew-based
link step without the Makefile, see the tap README
[local installation guide](https://github.com/tenfyzhong/homebrew-tap#installing-local-agentix-binaries).

Source and Formula files are never rewritten. Keep binary/resource inputs
unchanged while the snapshot is created. Artifact/profile content determines
the `0.0.0-local.<digest>.<profile>` Cellar version; uncompiled source changes
are excluded. Installed metadata remains readable after removing the checkout.

`install --skip-link` stages new artifacts without automatically linking them
or replacing installed stable/HEAD kegs. Once every selected install succeeds,
the target runs `switch VERSION=local` with the same profile and selection.
Switch validates all local kegs before unlinking the actual linked kegs, then
links the exact selected versions. Update passes the same artifact inputs into
switch so it selects this snapshot even when an older snapshot is reused and
another local version was installed more recently. A standalone switch without
local source inputs still selects the most recently installed matching profile.
This works even when `opt` points to a
different version from the command links. Identical artifacts are reused;
changed binary/resource content creates another local version. Failed installs
do not run switch and preserve existing command links. Homebrew can still
update `opt` paths during staging. This target does not restart services.
Restart only the selected service after installation:

```sh
brew services restart tenfyzhong/tap/agentix
brew services restart tenfyzhong/tap/taskix
```

Use `make switch VERSION=stable` or `make switch VERSION=head` to return to an
already installed upstream version. If it is missing, run
`make update VERSION=stable` or `make update VERSION=head` to install it again.
Directly with Homebrew, run `brew reinstall` or
`brew reinstall --HEAD` without `HOMEBREW_AGENTIX_LOCAL_SOURCE`, then restart
the selected service explicitly. To rebuild the local source again, rerun
`make update VERSION=local`.

## Installing and switching Homebrew versions

The following targets manage both `agentix` and `taskix` from
`tenfyzhong/tap`. Use `FORMULAE=agentix` or `FORMULAE=taskix` to select
one CLI. `VERSION` defaults to `stable`.

| Operation | Command |
| --- | --- |
| Install and use the latest release | `make install` |
| Update and use the latest release | `make update` |
| Install and use HEAD | `make install VERSION=head` |
| Update and use the latest HEAD | `make update VERSION=head` |
| Build and install local release binaries | `make update VERSION=local` |
| Build and install local debug binaries | `make update VERSION=local PROFILE=debug` |
| Switch to an installed release | `make switch` |
| Switch to an installed HEAD | `make switch VERSION=head` |
| Switch to the latest installed local release build | `make switch VERSION=local` |
| Switch to the latest installed local debug build | `make switch VERSION=local PROFILE=debug` |

For stable and HEAD, install and update refresh Homebrew metadata, install the
explicitly selected version with `--skip-link`, then switch command links.
Release updates use
`brew install` to select the current stable formula even when HEAD is installed;
HEAD updates additionally use `--fetch-HEAD` to check upstream commits.
An already current installation is reusable. Switch only checks installed kegs
and changes links; it does not download or build. Every selected CLI must have
the requested version installed before any command is unlinked. Local switch
selects the most recently installed local keg for each CLI and the requested
profile, using its Homebrew installation receipt time. It links that exact keg
and updates `opt`; local builds do not satisfy a stable-version request.
Switching uses Homebrew's Ruby unlink operation on the actual linked keg, because
`brew unlink` by formula can select the new `opt` keg while commands still link to the old one.

These targets do not restart services. Homebrew installation can change the
`opt` path even with `--skip-link`; that flag only defers ordinary command
linking. If you run Agentix as a Homebrew service, restart it explicitly after
installation, update, or switching:

```sh
brew services restart tenfyzhong/tap/agentix
brew info agentix taskix
agentix --version
taskix --version
```

Installation failures stop before the explicit switch. A link failure is
reported without attempting rollback. Existing versions are not explicitly
uninstalled, but Homebrew's own cleanup policy still applies. The targets do not
overwrite arbitrary files when Homebrew reports a link conflict.

## Installing plugins

Run `make plugin` (or `make plugin SOURCE=main`) to install from GitHub.
Run `make plugin SOURCE=local` to install from the current checkout. Both commands
default to all four hosts: Codex, Pi, OMP, and Claude Code. Select one or more
with `HOSTS`:

```sh
make plugin HOSTS=pi
make plugin SOURCE=main HOSTS="codex omp claude"
make remove-plugin HOSTS="pi omp"
```

| Host | Installed integration | Remote source |
| --- | --- | --- |
| Codex | Taskix Manager | GitHub marketplace with `--ref main` |
| Pi | Repository package containing Taskix Manager and Agentix Bridge | `git:github.com/tenfyzhong/agentix@main` |
| OMP | Taskix Manager and Agentix Bridge | GitHub marketplace using the repository's default branch, currently `main` |
| Claude Code | Taskix Manager and Agentix Bridge | GitHub marketplace with `@main` |

OMP's marketplace CLI does not expose a branch selector. If the repository's
default branch changes, its remote source handling must be updated accordingly.
Local marketplace paths use `./`, which OMP requires for relative paths.

All selected host CLIs must be on PATH; missing commands, unknown hosts, an empty selection,
or an invalid source fail before cleanup. Unselected hosts are untouched.

Installation first removes the selected hosts' Agentix registrations.
Cleanup is best-effort so absent plugins do not prevent setup. OMP also removes
the legacy `agentix-plugins` package. Pi cleanup covers the current checkout and
the unqualified and `@main` Git sources. If Pi still has another checkout
registered, remove it explicitly with `pi remove /path/to/old/checkout`;
project-local registrations require `-l` in that project.

The recipes do not build or replace CLI binaries or configure Obsidian.
Taskix Manager uses Node built-ins and plain JSON Schema for Pi/OMP tools.
Users do not need to install npm dependencies in the host cache.
Installation failures stop the target and are reported; earlier host changes
are not rolled back. Repeated installations are supported. Restart or reload
the hosts after installation. Start a new Codex thread and review/trust hooks
with `/hooks`; see the [plugin installation guide](https://github.com/tenfyzhong/agentix/wiki/Taskix-Manager#prerequisites-and-activation).

## Tests and external dependencies

Place tests near the boundary they exercise:

- Pure parsing, rendering, and protocol mappings belong in focused unit or adapter tests.
- Routing, persistence, lifecycle, command, approval, and interaction behavior belongs in `agentix-core` orchestration tests.
- Codex protocol sequences belong in the stateful mock app-server suite under `crates/agentix-codex/tests/`.
- Telegram and Feishu transport behavior belongs in their adapter integration suites and should use the in-process Bot API or OpenAPI/WebSocket mocks.
- Pi and Oh My Pi subprocess behavior should use reusable fake RPC processes.

Do not make automated tests depend on live credentials, public network services, a developer's session data, or an already running daemon. Extend the relevant mock when a new external API method, event, error, or state transition is introduced. Keep mock payloads aligned with the upstream wire format used by Agentix.

Run the complete quality gate to check formatting, run Clippy, and execute the workspace tests:

```sh
make check
```

This is equivalent to:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
node --test plugins/taskix-manager/tests/*.test.mjs
python3 -m unittest discover -s scripts/tests -v
```

Taskix Manager has no third-party npm dependencies. The taskix integration suite imports the actual Pi/OMP TypeScript entrypoints directly. Node.js 24+ and npm are required for the tests, including offline package fixtures.

The optional desktop Obsidian smoke test is ignored by default and requires an explicitly selected open test vault. It creates and removes only its own temporary files and tab. Bring the selected vault window to the foreground before running it. See [task board validation](docs/task-board.md#validation) for the command and the [integration coverage map](docs/integration-coverage.md) for automated boundaries and separate live-system acceptance.

The test suite is layered. Protocol and rendering tests cover pure mappings; adapter tests cover Telegram, Feishu, Pi, and Codex transports; and core tests exercise routing, persistence, actions, interactions, and lifecycle transitions. Full-stack tests pass mocked Telegram and Feishu events through the channel adapter, engine, and Codex client before verifying the completed response at the channel API.

Codex uses a stateful mock app-server under `crates/agentix-codex/tests/support/`. It follows the Codex CLI 0.153.0 protocol subset used by Agentix, including session lifecycle, settings, approvals, input questions, pagination, failures, and reconnects. Telegram and Feishu use in-process API services, Pi uses a reusable fake RPC subprocess, and rmux tests exchange typed SDK packets with a Unix-socket mock daemon. These fixtures keep the suite deterministic and independent of live credentials, public networks, local session data, and running daemons.

Channel shutdown deadline tests use Tokio's paused clock to verify the shared grace period and task cancellation independently of database and filesystem latency. Service lifecycle tests also exercise startup and shutdown with a temporary SQLite database. The test suite checks the non-Unix Codex compatibility API on Unix hosts as well, so Windows-only API omissions are caught locally.

GitHub Actions keeps formatting and Clippy in `ci.yml`. The `tests.yml` workflow runs the full suite in two cargo-nextest hash partitions on Linux and four on macOS. Windows runs four partitions of the memory, task library and taskix suites; an independent Windows platform job checks the workspace and runs the native TCP control tests. CI installs cargo-nextest 0.9.146 and uses the `ci` profile to start the slow native plugin integration first. The short memory IPC suite and the bounded memory triage metrics integration reserve all test threads to protect their real deadlines from competing tests; local `make check` continues to use Cargo's standard test runner. Doctests run separately once per platform because nextest does not run them. Task timestamp tests use `TZ` overrides on Unix; the Windows platform job switches the native system time zone after its other tests to verify UTC+09:00, UTC-05:00, and UTC, then restores the original setting. Each partition uses a separate runner. Both workflows cancel superseded runs and run for pull requests and pushes to `main`, except when every changed file is Markdown (`.md`). Changes that include any other file still run both workflows. The Tests workflow also supports manual dispatch regardless of the changed files. See [CI test cost](docs/integration-coverage.md#ci-test-cost) for coverage guards and timing boundaries.

## Workspace architecture

The main crates are `agentix-domain` (contracts), `agentix-storage` (runtime persistence), `agentix-core` (application services), `agentix-codex`, `agentix-bridge`, `agentix-telegram`, `agentix-feishu`, the independent `agentix-task` library, and the `agentix` / `taskix` executables. See [task board design and usage](https://github.com/tenfyzhong/agentix/wiki/Taskix) for the task database and document projection boundary.

The core exposes a small common agent interface plus optional queue, attached-session control, and workspace-runtime ports. A serialized runtime loop feeds IM and agent events into coordinator-owned session, turn, interaction, and rmux state. See the [architecture document](docs/architecture.md) for the state/effect and retry boundaries.

Run `make` for a debug build, `make release` for a release build, or `make help` to list the available targets.

Both CLIs append the current Git commit to development versions, for example
`0.0.0-dev+dd207c5abc12`. Uncommitted tracked or untracked changes add `.dirty`;
ignored build artifacts do not. Cargo refreshes this metadata on each build,
including optimized builds and `brew install --HEAD`. Versions ending in `-dev`
use this rule; tagged releases retain their exact release version. Development
source archives without Git metadata report `0.0.0-dev+unknown`.

## Code and documentation style

- Follow rustfmt and the workspace Clippy configuration.
- Treat warnings as errors.
- Do not add unsafe Rust; the workspace forbids it.
- Keep abstractions at transport boundaries so orchestration can be tested independently.
- Preserve existing user changes and avoid unrelated rewrites.
- Use consistent indentation and leave no trailing whitespace.
- Use English for repository documentation, code comments, identifiers, user-facing messages, and test descriptions and fixture labels.
- Preserve Unicode test coverage with English labels and escaped Unicode symbols rather than non-English prose. Unicode punctuation and interface icons do not need to be ASCII.

## User documentation and Wiki maintenance

User-facing installation, configuration, and usage guides are maintained in the [GitHub Wiki](https://github.com/tenfyzhong/agentix/wiki). Keep the repository's `docs/` directory focused on architecture, protocols, test coverage, benchmarks, and implementation reviews; update its [index](docs/README.md) when adding technical documents.

When behavior changes, update the corresponding Wiki guide as well as any affected technical documents. The Wiki has a separate Git repository:

```sh
git clone git@github.com:tenfyzhong/agentix.wiki.git
cd agentix.wiki
git pull --ff-only
git worktree add .git/wtm/docs/wiki-update -b docs/wiki-update
cd .git/wtm/docs/wiki-update
# Edit the relevant Markdown pages and check page links and anchors.
git add -- '*.md'
git commit -s -m "docs: update user guide"
# Review the commit before publishing. Wiki changes go live on master.
git push origin HEAD:master
```

Keep `Home.md` and `_Sidebar.md` aligned with page names. Use full Wiki URLs for page links and full repository URLs for source files. Delete obsolete user-guide files after migrating their content to the Wiki; do not keep redirect-only placeholders or duplicate usage instructions. Move any remaining technical chapters into the appropriate architecture, protocol, coverage, or contribution document before deleting an old guide. Update repository links, configuration comments, and package resource expectations when removing a file.

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
3. verifies each binary's `--version` against the tag; macOS/Linux jobs immediately reuse their binaries for CLI bottles and install the tagged backup script for its separate bottle, without waiting for Windows;
4. tests the standalone backup script and publishes `taskix-backup-<tag>.tar.gz`, separate `agentix-<tag>-<target>` and `taskix-<tag>-<target>` archives, a shared `SHA256SUMS`, and generated notes to the matching GitHub Release;
5. publishes the prepared Homebrew formula PRs after all verified bottles and native archives are uploaded.

Each CLI archive includes its own binary, example configuration, and shell completions. Only the taskix archive includes task documentation and the taskix-manager plugin. All targets have `.tar.gz` archives; Windows additionally has `.zip` archives for both CLIs. The architecture-independent backup archive contains the executable `taskix-backup` script and `LICENSE`; it requires Python 3.11+ and rclone on macOS/Linux. Packaging tests execute the workflow's packaging and checksum steps against fixtures to verify archive contents and separation.

The Agentix Homebrew formula is maintained in [`tenfyzhong/homebrew-tap`](https://github.com/tenfyzhong/homebrew-tap/blob/main/Formula/agentix.rb); edit dependencies, installation steps, and service settings there. Keep formulas exclusively in the tap repository. Shared preparation updates the source tag URL and checksum and removes stale bottle metadata. CI runs `brew trust` before `brew tap`: tapping can immediately validate formulae, so deferring trust until afterward fails on a fresh runner. Each macOS/Linux release job then installs its prebuilt binaries through a temporary formula overlay, preserving the tap's completion, configuration, service, and test definitions. The overlay skips Cargo and its compiler dependencies; the original source formula is restored in both the tap and installed keg before bottling. Source and HEAD installation recipes remain unchanged in the published formula.

Bottles are built and tested on macOS arm64, Linux x86_64, and Linux arm64. Each bottle must pass version checks, `brew linkage --test`, and `brew test` both before and after uninstalling the staging installation and pouring the exported local bottle. Release publication waits for every platform; the subsequent tap PR step only merges bottle metadata and never recompiles. Linux builders are pinned to Ubuntu 24.04 for both native archives and bottles; reuse does not establish compatibility with older glibc versions. macOS compatibility is limited by the native binary's deployment target and the bottle's OS tag.

Manual `Homebrew` dispatch remains available for an existing release and selected formula (`all`, `agentix`, `taskix`, or `taskix-backup`). It downloads the selected CLI archives, verifies each against `SHA256SUMS`, and uses the same bottle packaging and installation tests. The backup formula installs the checksummed tag source directly and requires no native archive download. Missing or mismatched checksums fail before extraction. Automatic and manual publishing require a `HOMEBREW_TAP_TOKEN` with permission to create branches and pull requests.

For a release blocked by packaging automation, merge the fix and dispatch `Release` from `main` with the existing `tag` input. The binaries still build from that tag, while packaging uses the dispatch workflow commit. Rerunning the original tag-push run keeps its original workflow commit and cannot pick up packaging fixes made later. macOS CI backup packaging unlinks an installed `openssl@1.1` and removes a residual `bin/openssl` symlink only when it points to that legacy keg before installing dependencies to avoid conflicts with `openssl@3`; local invocations and Linux packaging retain their existing links.

Run the focused packaging regressions with `cargo test -p agentix --all-features --test packaging` and `node --test plugins/taskix-manager/tests/homebrew.test.mjs plugins/taskix-manager/tests/release-bottles.test.mjs plugins/taskix-manager/tests/release-backup.test.mjs`. On a machine with Homebrew and a C compiler, `AGENTIX_TEST_HOMEBREW=1 node --test plugins/taskix-manager/tests/release-bottles.test.mjs` also exercises a real bottle install/export/pour using a uniquely named disposable formula. It does not replace installed Agentix or Taskix packages. In the tap worktree, run `AGENTIX_TEST_HOMEBREW=1 AGENTIX_TEST_SOURCE=/absolute/path/to/agentix node --test tests/backup.test.mjs` to install, export and pour the actual backup Formula recipe with an isolated SQLite/rclone round trip. It leaves installed production commands and services alone.

When the source URL changes for a new tag, the Homebrew workflow also removes the formula's old `revision` so the new upstream version starts at revision zero. Re-running the same tag preserves its revision while rebuilding the bottle.

Before tagging a release:

1. Run formatting, Clippy, all tests, and documentation tests.
2. Run `agentix doctor` as the intended runtime user.
3. Verify the selected channel's owner allowlist and group mention behavior.
4. Exercise concurrent sessions and confirm their cards update independently.
5. Restart Agentix during an active turn and verify that the original message recovers its Stop action and completion sends a fresh final card.
6. Restart the Codex daemon and verify reconnect and subscription recovery.
7. Attach a fresh Codex TUI before its first prompt, send that prompt from IM, and verify that the session materializes and resumes.

### Release packaging

The Homebrew formulae for Agentix, Taskix and the independent `taskix-backup` tool are maintained in [tenfyzhong/homebrew-tap](https://github.com/tenfyzhong/homebrew-tap). Release automation updates all three formulae and publishes macOS arm64, Linux x86_64, and Linux arm64 bottles. Each platform compiles the CLIs once and reuses those binaries for both release archives and Homebrew bottles. The backup formula installs tagged Python source with Homebrew Python and rclone, without a native binary overlay or Rust dependencies. Its Formula test uploads to an isolated local rclone remote and verifies a restored SQLite row. Bottle packaging preserves the prepared source formula and verifies a real bottle installation; after every platform succeeds, automation merges the metadata into one pull request per formula. Install Taskix with `brew install tenfyzhong/tap/taskix` and its optional backup tool separately with `brew install tenfyzhong/tap/taskix-backup`. Until the first stable backup Formula is published, use `brew install --HEAD tenfyzhong/tap/taskix-backup`. The Homebrew workflow can also be run manually for an existing release tag, selecting any of these formulae or `all` (the default) to package the corresponding bottles and publish formula pull requests.

### Taskix Homebrew formula

Maintain `Formula/taskix.rb` exclusively in `tenfyzhong/homebrew-tap`.
The local tap repository is `/opt/homebrew/Library/Taps/tenfyzhong/homebrew-tap`;
use its dedicated branch worktree for edits. Agentix contains no formula template.
The release workflow checks out the tap and updates its formula with the published
tag URL and source archive SHA-256, then packages the prebuilt Taskix binary, tests the bottle, and opens a tap PR.
It also supports adding the first stable release to a HEAD-only tap formula.

Merge the Taskix source change and tap formula before publishing the first
Taskix release. Until that release formula is published, use
`brew install --HEAD tenfyzhong/tap/taskix` after the source rename is merged.
The release workflow requires `HOMEBREW_TAP_TOKEN` with permission to create
formula update PRs in the tap repository.
