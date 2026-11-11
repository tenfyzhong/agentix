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
