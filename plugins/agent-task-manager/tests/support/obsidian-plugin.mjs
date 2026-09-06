import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import vm from "node:vm";

export function loadPlugin(obsidian = {}, globals = {}, modules = {}) {
    const module = { exports: {} };
    const require = createRequire(import.meta.url);
    const filename = new URL("../../obsidian/taskcli-sync/main.js", import.meta.url);
    vm.runInNewContext(readFileSync(filename, "utf8"), {
        module, exports: module.exports, console, setTimeout, clearTimeout,
        require: (id) => id === "obsidian" ? {
            Plugin: class {}, PluginSettingTab: class {}, ...obsidian,
        } : modules[id] || require(id),
        ...globals,
    }, { filename: filename.pathname });
    return module.exports;
}

export const copy = (value) => JSON.parse(JSON.stringify(value));

export async function fixture(overrides = {}) {
    const { SyncEngine } = loadPlugin();
    const row = {
        kind: "task", id: "task_one", project_id: "prj_one", path: "Tasks/Projects/Demo/Tasks/One.md",
        revision: 1, status: "TODO", properties: { status: "TODO", completedDate: null },
    };
    const state = { documents: { format: "obsidian", root: "/vault", directory: "Tasks" }, notes: [row] };
    const files = new Map([[row.path, { id: row.id, task_id: row.id, revision: 1, status: "TODO", completedDate: null, custom: "keep" }]]);
    const calls = [], notices = [];
    let engine;
    const io = {
        snapshot: async () => copy(state),
        read: async (path) => copy(files.get(path)),
        patch: async (path, expected, properties) => {
            const file = files.get(path);
            if (!file || file.id !== expected.id || file.revision !== expected.revision || JSON.stringify(file.status) !== JSON.stringify(expected.status)) return false;
            Object.assign(file, copy(properties));
            engine.observe(path, copy(file));
            return true;
        },
        execute: async (args) => {
            calls.push(copy(args));
            const command = args[1];
            const status = { block: "BLOCKED", wait: "WAITING_USER", cancel: "CANCELLED", retry: "TODO", reopen: "TODO" }[command];
            if (!status) throw new Error("Unsupported fixture command");
            row.status = status;
            row.revision++;
            row.properties.status = status;
            Object.assign(files.get(row.path), { status, revision: row.revision });
            engine.observe(row.path, copy(files.get(row.path)));
            return { ok: true, result: copy(row), projection_pending: null };
        },
        notice: (message) => notices.push(message),
        ...overrides,
    };
    engine = new SyncEngine(io);
    await engine.initialize();
    return {
        engine, io, state, row, files, calls, notices,
        edit(status) {
            const file = files.get(row.path);
            file.status = status;
            engine.observe(row.path, copy(file));
        },
    };
}
