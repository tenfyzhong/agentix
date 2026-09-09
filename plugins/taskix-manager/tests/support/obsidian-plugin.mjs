import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import vm from "node:vm";

export function loadPlugin(obsidian = {}, globals = {}, modules = {}) {
    const module = { exports: {} };
    const require = createRequire(import.meta.url);
    const filename = new URL("../../obsidian/taskix-sync/main.js", import.meta.url);
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
        revision: 1, status: "TODO", properties: { status: "TODO", completed_at: null },
    };
    const state = { documents: { root: "/vault", directory: "Tasks" }, notes: [row] };
    const files = new Map([[row.path, { id: row.id, task_id: row.id, revision: 1, status: "TODO", completed_at: null, custom: "keep" }]]);
    const calls = [], notices = [];
    let engine;
    const io = {
        connection: async () => ({ documents: copy(state.documents) }),
        lookup: async (id) => copy(state.notes.find((note) => note.id === id) || null),
        openNotes: () => [...files].map(([path, properties]) => ({ path, properties: copy(properties) })),
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

export async function connectionFixture(directory = "11-Agents") {
    const notices = [], requests = [], commands = [], buttons = [];
    const vaultEvents = new Map(), metadataEvents = new Map();
    class TFile {}
    const file = new TFile();
    let settingsTab;
    const Plugin = loadPlugin({
        TFile,
        parseYaml: JSON.parse,
        PluginSettingTab: class { constructor() { this.containerEl = { empty() {} }; } },
        Notice: class {
            constructor(message) { this.message = message; this.hidden = false; notices.push(this); }
            hide() { this.hidden = true; }
        },
        Setting: class {
            setName() { return this; }
            setDesc() { return this; }
            addText(callback) {
                callback({ setValue() { return this; }, onChange() { return this; } });
                return this;
            }
            addButton(callback) {
                const button = {
                    setButtonText(value) { this.text = value; return this; },
                    setDisabled(value) { this.disabled = value; return this; },
                    onClick(handler) { this.click = handler; return this; },
                };
                buttons.push(button); callback(button); return this;
            }
        },
    }, {}, {
        "node:fs": { realpathSync: (path) => path },
        "node:child_process": {
            execFile(binary, args, options, callback) {
                requests.push({ binary, args, callback });
                return { kill() {} };
            },
        },
    });
    const plugin = new Plugin();
    plugin.app = {
        vault: { adapter: { getBasePath: () => "/vault" }, on: (name, handler) => vaultEvents.set(name, handler), getAbstractFileByPath() { return null; } },
        metadataCache: { on: (name, handler) => metadataEvents.set(name, handler) },
        workspace: { onLayoutReady() {}, on() {}, getLeavesOfType() { return []; } },
    };
    plugin.loadData = async () => ({ cliPath: "/bin/taskix", configPath: "/config.toml" });
    plugin.addSettingTab = (tab) => { settingsTab = tab; };
    plugin.addCommand = (command) => commands.push(command);
    plugin.registerEvent = () => {};
    plugin.registerBasesView = () => {};
    await plugin.onload();
    settingsTab.display();
    return {
        plugin, file, notices, requests, commands, vaultEvents, metadataEvents, button: buttons[0],
        reply(error, index = requests.length - 1, result) {
            result ??= requests[index].args.includes("show") ? null : { protocol_version: 1, documents: { root: "/vault", directory } };
            requests[index].callback(error ? new Error(error) : null, JSON.stringify(error
                ? { schema_version: 1, ok: false, error: { message: error } }
                : { schema_version: 1, ok: true, result }), "");
        },
    };
}
