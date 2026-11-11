# Install plugins without a checkout

Install Agentix, Taskix, Node.js, and the host CLI first. Run the commands for your
host from any directory; cloning Agentix and running Make are unnecessary.
Hosts download and maintain their own plugin caches.

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
