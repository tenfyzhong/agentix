# Install plugins without a checkout

Install Agentix, Taskix, Node.js, and the host CLI first. Run the commands for your
host from any directory; cloning Agentix and running Make are unnecessary.
Hosts download and maintain their own plugin caches. This guide covers all six
supported hosts: Codex, Claude Code, Pi, OMP, OpenCode, and Antigravity.

| Host | Installed components |
| --- | --- |
| Codex | Taskix Manager; Agentix connects through Codex app-server |
| Claude Code | Taskix Manager and Agentix Bridge |
| Pi | Taskix Manager and Agentix Bridge in one package |
| OMP | Taskix Manager and Agentix Bridge |
| OpenCode | Taskix server plugin and Agentix Bridge TUI plugin |
| Antigravity | Taskix Manager and Agentix Bridge native plugins |

## Codex

Install Taskix Manager from the marketplace:

```sh
codex plugin marketplace add tenfyzhong/agentix --ref main
codex plugin add taskix-manager@agentix
codex plugin list
```

Codex does not need the Agentix Bridge plugin: Agentix connects through its
app-server integration. Start a new Codex thread after installation, and review
and trust the Taskix hooks with `/hooks`.

To refresh the marketplace and installed plugin:

```sh
codex plugin marketplace upgrade agentix
codex plugin add taskix-manager@agentix
```

To remove it:

```sh
codex plugin remove taskix-manager@agentix
codex plugin marketplace remove agentix
```

## Claude Code

Install both plugins:

```sh
claude plugin marketplace add tenfyzhong/agentix@main
claude plugin install taskix-manager@agentix
claude plugin install agentix-bridge@agentix
claude plugin list
```

These commands use the default user scope. Restart Claude Code after installation
or update so its hooks and bridge load in the original session.

To update:

```sh
claude plugin marketplace update agentix
claude plugin update taskix-manager@agentix
claude plugin update agentix-bridge@agentix
```

To remove:

```sh
claude plugin uninstall taskix-manager@agentix
claude plugin uninstall agentix-bridge@agentix
claude plugin marketplace remove agentix
```

## Pi

Install the repository package, which includes both host extensions and the
shared Taskix skill:

```sh
pi install git:github.com/tenfyzhong/agentix@main
pi list
```

Restart Pi after installation or update to load the extensions. The default is a
user installation; add `-l` to install only for the current project.

To update:

```sh
pi update git:github.com/tenfyzhong/agentix@main
```

To remove:

```sh
pi remove git:github.com/tenfyzhong/agentix@main
```

Use the same source string that was installed. For project-local installations,
run the removal command with `-l` from that project.

## OpenCode 2.0.32+

```sh
opencode plugin add 'github:tenfyzhong/agentix#main::path:plugins/taskix-manager'
opencode plugin add 'github:tenfyzhong/agentix#main::path:plugins/agentix-bridge'
```

Use `opencode plugin update` to update. Restart the TUI and its background server
(`opencode service restart`) after finishing active work. The native installer
registers the server and TUI exports in their respective configurations.

## Antigravity CLI

```sh
agy plugin install https://github.com/tenfyzhong/agentix/tree/main/plugins/taskix-manager
agy plugin install https://github.com/tenfyzhong/agentix/tree/main/plugins/agentix-bridge
```

Repeat the commands to update and restart agy. See [Antigravity](antigravity.md)
for configuration and removal.

## OMP

```sh
omp plugin marketplace add tenfyzhong/agentix
omp plugin install taskix-manager@agentix
omp plugin install agentix-bridge@agentix
```

To update, run `omp plugin marketplace update agentix`, then repeat both install
commands with `--force`. Restart OMP to load the extensions. To remove them, use
`omp plugin uninstall taskix-manager@agentix` and
`omp plugin uninstall agentix-bridge@agentix`.

## Existing installations and development

Do not keep legacy local and remote copies enabled together. Remove old
registrations using the host's plugin manager before switching sources.
Developers can use `make plugin HOSTS=opencode` in a checkout to migrate old
OpenCode registrations while preserving unrelated settings.

The Make targets remain developer conveniences for published branches. OMP's
native marketplace command follows the repository default branch; its non-main
developer helper retains a managed branch checkout because the native command
has no branch selector. Ordinary main installations use native commands directly.

## Verification

The opt-in Node test uses isolated homes and starts in an empty directory:

```sh
AGENTIX_TEST_NATIVE_REMOTE_INSTALL=1 node --test plugins/taskix-manager/tests/native-remote-install.test.mjs
```

It verifies native downloads and registrations from published main. Local package
and installer regression tests verify unpublished changes; remote tests do not
claim that those changes have been published or that model conversations passed.

## Unified release versions

The tooling in this section is introduced by the unified-version change; use a
checkout containing it until the first release with this policy is published.

Agentix, Taskix, both host plugins, and Taskix Sync share one release version.
The commands above use `main`, a development channel. To install a stable release,
select a published numeric tag from [Releases](https://github.com/tenfyzhong/agentix/releases)
and replace `main` in the Codex, Claude, Pi, OpenCode and agy source references
with that exact tag. Upgrade the two CLIs and plugins together. Releases before
this policy was introduced retain their historical independent plugin versions.

A tag is pinned: refreshing it does not move to the next release. To upgrade,
remove the old marketplace/source registration using the commands above, then
install the new tag. For Pi, remove the exact old `git:...@TAG` source before
installing the new one to avoid duplicate extensions. Keep unrelated plugins.
OMP's native marketplace command follows main; use the checkout helper below
for a fixed tag, since OMP has no native marketplace ref selector.

With a checkout containing the unified-version tooling:

```sh
make plugin-stable HOSTS=codex
make plugin PLUGIN_TAG=1.2.3 HOSTS="pi claude"
make remove-plugin PLUGIN_TAG=1.2.3 HOSTS=pi
```

`1.2.3` is an example, not a claim that this tag exists. `plugin-stable` resolves
GitHub's latest stable release once, then passes the same numeric tag to every
selected host. An unavailable tag fails before cleanup. With no `PLUGIN_TAG`,
`make plugin` retains its current remote branch behavior. OMP uses its existing
managed remote cache for fixed tags as well as non-main branches.

### Inspect the installed version

Use your host's plugin listing to locate its installed Taskix Manager directory:

```sh
node /path/to/installed/taskix-manager/version.mjs --check
```

The command reads that directory, not the current working checkout. It reports
version, source URL, branch/tag, full commit SHA, dirty state, latest release, and
update status. Without `--check` it works offline; `--json` returns structured
output. To inspect an older plugin without this script, or Agentix Bridge or
Taskix Sync, use the new script with `--path /actual/plugin/directory`. From a
checkout the equivalent is:

```sh
make plugin-status PLUGIN_PATH=/path/to/installed/taskix-manager
```

A source checkout is not proof of an installed or active host plugin. Choose the
actual directory reported by the host, and restart that host after updating.
Caches without Git metadata report an unknown source/commit; equal manifest
versions alone do not prove current code. Branch checks compare its local SHA
with the same remote branch; stable tag checks compare release versions. Dirty
sources report `modified`. Missing metadata, deleted branches, API rate limits
and network failures report `unknown`, never `current`. This command is read-only
and does not refresh caches or change host registrations.

### Preparing a release

Maintainers use `VERSION` as the single source of truth. On a release branch:

```sh
make prepare-release VERSION=1.2.3
make check-versions
```

Review and commit the generated Cargo, package, lockfile and plugin manifest
changes before creating the matching numeric tag. Release CI verifies the
committed versions before building or publishing. PR CI catches partial version
changes. `prepare-release` does not commit, tag, push or publish. After a release,
reset all development versions together with `make prepare-release VERSION=0.0.0-dev`;
development identity comes from its branch and commit, not this placeholder.
Obsidian stable releases use the same numeric version. Protocol/schema versions
and third-party dependency versions remain independent.
