# Bundled MCP SDK

This is the same pinned MCP SDK bundle and license inventory as
`plugins/agentix-bridge/claude/vendor`. Bundling it here keeps Taskix independently
installable, without requiring the Agentix bridge plugin or an npm install at runtime.

To regenerate it, run `node claude/build.mjs` from `plugins/agentix-bridge`, then
copy `claude/vendor/sdk.mjs` and `claude/vendor/LICENSES.md` into this directory.
