"use strict";

const { Plugin, PluginSettingTab, Setting, Notice, TFile, parseYaml } = require("obsidian");
const { execFile } = require("node:child_process");
const { randomUUID } = require("node:crypto");
const { realpathSync } = require("node:fs");
const path = require("node:path");

function localTimestamp(seconds) {
    const date = new Date(seconds * 1000);
    const offset = -date.getTimezoneOffset();
    const pad = value => String(Math.abs(value)).padStart(2, "0");
    return new Date(date.getTime() + offset * 60000).toISOString().replace(/\.000Z$/, "") +
        `${offset < 0 ? "-" : "+"}${pad(Math.trunc(offset / 60))}:${pad(offset % 60)}`;
}

function commandFor(note, target) {
    if (note.kind === "inbox" && ["TODO", "ACTIVE", "PENDING_REVIEW", "COMPLETED", "CANCELLED"].includes(target)) {
        return ["inbox", "set-status", note.id, "--status", target];
    }
    const reason = `Status changed in Obsidian: ${note.status} -> ${target}`;
    let command;
    if (note.kind === "task") {
        command = { BLOCKED: "block", WAITING_USER: "wait", FAILED: "fail", CANCELLED: "cancel" }[target];
        if (target === "TODO") {
            command = note.status === "FAILED" ? "retry" : ["DONE", "CANCELLED"].includes(note.status) ? "reopen" : undefined;
        }
        if (["IN_PROGRESS", "DONE"].includes(target)) {
            throw new Error("Use taskcli claim, plan and start/done with the owning session; this plugin does not hold task leases.");
        }
    } else if (note.kind === "job") {
        if (target === "CANCELLED") command = "cancel";
        if (note.status === "ACTIVE" && target === "PENDING_REVIEW") command = "submit";
        if (note.status === "PENDING_REVIEW") {
            if (target === "ACTIVE") command = "reject";
            if (target === "COMPLETED") command = "approve";
        }
    }
    if (typeof target !== "string" || typeof command !== "string") {
        throw new Error(`Unsupported ${note.kind} transition: ${note.status} -> ${String(target)}`);
    }
    const args = [note.kind, command, note.id];
    if (["block", "wait", "fail", "reject"].includes(command)) args.push("--reason", reason);
    return args;
}

const equal = (a, b) => JSON.stringify(a) === JSON.stringify(b);

// Offsets refer to the original document so rollback can preserve every byte
// outside this entry's checkbox and generated state receipt.
function parseInbox(source, project) {
    const starts = [...source.matchAll(/^<!-- taskcli:inbox:start project=([^\r\n]+) -->\r?$/gm)];
    const ends = [...source.matchAll(/^<!-- taskcli:inbox:end -->\r?$/gm)];
    if (starts.length !== 1 || ends.length !== 1 || starts[0].index >= ends[0].index ||
        (project && starts[0][1] !== project)) throw new Error("Invalid Inbox project or managed region");
    const rows = [], ids = new Set();
    const start = starts[0].index + starts[0][0].length;
    const region = source.slice(start, ends[0].index);
    let fence;
    for (const line of region.matchAll(/[^\n]+/g)) {
        const text = line[0].replace(/\r$/, "");
        const delimiter = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(text);
        if (fence) {
            if (delimiter && delimiter[1][0] === fence[0] && delimiter[1].length >= fence.length && !delimiter[2].trim()) fence = null;
            continue;
        }
        if (delimiter) { fence = delimiter[1]; continue; }
        if (!text.startsWith("- [")) continue;
        const identity = / <!-- taskcli:entry:(inbox_[0-9a-f]{32}) -->/g;
        const matches = [...text.matchAll(identity)];
        if (matches.length === 0) {
            if (text.includes("<!-- taskcli:entry:")) throw new Error("Invalid Inbox entry identity");
            continue; // New submissions are registered by inbox sync.
        }
        if (matches.length !== 1 || ids.has(matches[0][1])) throw new Error("Duplicate Inbox entry identity");
        ids.add(matches[0][1]);
        const receipt = / <!-- taskcli:entry-state (TODO|ACTIVE|PENDING_REVIEW|COMPLETED|CANCELLED|IN_PROGRESS|DONE)(?: revision=(\d+))? -->/.exec(text);
        if (!receipt || !/^- \[.\] /.test(text)) throw new Error("Invalid Inbox status receipt or checkbox");
        const checkbox = text[3];
        const status = { " ": "TODO", "/": "ACTIVE", r: "PENDING_REVIEW", p: "PENDING_REVIEW", x: "COMPLETED", X: "COMPLETED", "-": "CANCELLED" }[checkbox] || checkbox;
        rows.push({ id: matches[0][1], project_id: starts[0][1], status, revision: Number(receipt[2] || 0),
            checkboxOffset: start + line.index + 3,
            receiptOffset: start + line.index + receipt.index, receiptLength: receipt[0].length });
    }
    if (fence) throw new Error("Unclosed Inbox code fence");
    return rows;
}

function patchInbox(source, note, expected, properties) {
    const row = parseInbox(source, note.project_id).find((entry) => entry.id === note.id);
    if (!row || row.id !== expected.id || row.revision !== expected.revision || row.status !== expected.status) return source;
    const checkbox = { TODO: " ", ACTIVE: "/", PENDING_REVIEW: "r", COMPLETED: "x", CANCELLED: "-" }[properties.status];
    if (!checkbox) throw new Error(`Invalid Inbox state: ${properties.status}`);
    const receipt = ` <!-- taskcli:entry-state ${properties.status} revision=${properties.revision} -->`;
    const next = source.slice(0, row.receiptOffset) + receipt + source.slice(row.receiptOffset + row.receiptLength);
    return next.slice(0, row.checkboxOffset) + checkbox + next.slice(row.checkboxOffset + 1);
}

const inboxKey = (filePath, id) => `${filePath}#${id}`;

function recentJobsView(app, controller, container) {
    // Resolve at view creation so TaskNotes can load after Taskcli Sync.
    const registration = app.internalPlugins?.plugins?.bases?.instance?.registrations?.tasknotesKanban;
    if (!registration?.factory) throw new Error("Recent Jobs requires the TaskNotes Kanban view. Enable TaskNotes and reopen this Base.");
    const view = registration.factory(controller, container);
    const extract = view.dataAdapter.extractDataItems.bind(view.dataAdapter);
    view.dataAdapter.extractDataItems = () => {
        // Bases already sorted these entries. Limit independently per status,
        // without changing shared query data or other TaskNotes views.
        const counts = new Map();
        return extract().filter(item => {
            const status = item.properties.status;
            const count = counts.get(status) || 0;
            counts.set(status, count + 1);
            return count < 10;
        });
    };
    return view;
}

class SyncEngine {
    constructor(io) {
        this.io = io;
        this.notes = new Map();
        this.pending = new Map();
        this.timers = new Map();
        this.generations = new Map();
        this.disposed = false;
        this.ready = false;
        this.running = null;
        this.inFlight = null;
        this.directory = null;
    }

    remember(note) {
        this.notes.delete(note.path);
        this.notes.set(note.path, note);
        while (this.notes.size > 128) this.notes.delete(this.notes.keys().next().value);
        return note;
    }

    async lookup(intent) {
        const note = await this.io.lookup(intent.id);
        if (!note) return null;
        if (note.id !== intent.id || note.kind !== intent.kind || !Number.isInteger(note.revision)) {
            throw new Error("Invalid taskcli note response");
        }
        if (note.path !== (intent.filePath || intent.path) ||
            (intent.project_id && note.project_id !== intent.project_id)) return null;
        const item = note.kind === "inbox" ? { ...note, filePath: note.path, path: inboxKey(note.path, note.id) } : note;
        if (!this.disposed && this.generations.get(intent.path) === intent.generation) this.remember(item);
        return item;
    }

    watches(filePath) {
        if (!this.ready || this.disposed || this.directory === null) return false;
        const relative = path.posix.relative(this.directory, filePath);
        return relative !== ".." && !relative.startsWith("../") && !path.posix.isAbsolute(relative);
    }

    async initialize() {
        this.ready = false;
        const { documents } = await this.io.connection();
        if (this.disposed) return;
        this.directory = documents.directory;
        this.notes.clear();
        this.ready = true;
        await this.reconcile();
    }

    async reconcile() {
        for await (const file of this.io.openNotes?.() || []) {
            this.observe(file.path, file.properties, true);
            if (file.source) this.observeInbox(file.path, file.source, true);
        }
        await this.flush();
    }

    observe(filePath, properties, reconcileOnly = false, inboxFile) {
        if (!this.ready || this.disposed || !properties) return;
        const kind = typeof properties.id === "string" && /^(task|job|inbox)_/.exec(properties.id)?.[1];
        if (!kind || !Number.isInteger(properties.revision) ||
            (kind === "task" && properties.task_id !== properties.id)) return;
        const queued = this.pending.get(filePath);
        const note = this.notes.get(filePath) || (queued?.status ? queued : undefined);
        if (note && properties.id !== note.id) return;
        if (reconcileOnly && (this.pending.has(filePath) || this.inFlight?.path === filePath)) return;
        if (this.inFlight?.path === filePath && properties.revision > this.inFlight.revision &&
            equal(properties.status, this.inFlight.target)) return;
        if (!reconcileOnly && note && properties.revision === note.revision &&
            equal(properties.status, note.status) && this.inFlight?.path !== filePath) {
            // One CLI write projects every note. Do not let that projection
            // discard a different note's already queued user edit.
            if (this.inFlight && this.pending.has(filePath)) return;
            this.pending.delete(filePath);
            this.generations.delete(filePath);
            clearTimeout(this.timers.get(filePath));
            this.timers.delete(filePath);
            return;
        }
        const previous = this.pending.get(filePath);
        if (previous && equal(previous.target, properties.status) &&
            previous.revision === properties.revision) return;
        const generation = (this.generations.get(filePath) || 0) + 1;
        this.generations.set(filePath, generation);
        this.pending.set(filePath, {
            ...note, kind, id: properties.id, path: filePath, filePath: inboxFile || note?.filePath,
            project_id: properties.project_id || note?.project_id,
            revision: properties.revision, target: properties.status, reconcileOnly,
            generation, key: randomUUID(), ready: false,
        });
        clearTimeout(this.timers.get(filePath));
        this.timers.set(filePath, setTimeout(() => {
            this.timers.delete(filePath);
            const intent = this.pending.get(filePath);
            if (intent) intent.ready = true;
            void this.drain();
        }, 300));
    }

    observeInbox(filePath, source, reconcileOnly = false) {
        if (!this.ready || this.disposed) return;
        const notes = [...this.notes.values()].filter((note) => note.kind === "inbox" && note.filePath === filePath);
        if (!notes.length && !source.includes("<!-- taskcli:inbox:start project=")) return;
        try {
            const rows = parseInbox(source, notes[0]?.project_id);
            const present = new Set(rows.map((row) => row.id));
            for (const note of [...notes, ...this.pending.values()]) {
                if (note.filePath === filePath && !present.has(note.id)) this.forget(note.path);
            }
            for (const row of rows) this.observe(inboxKey(filePath, row.id), row, reconcileOnly, filePath);
        } catch (error) {
            // Discard pending intents from a malformed file; never guess which
            // duplicate ID to update or treat a broken region as withdrawal.
            this.forget(filePath);
            this.notify(`Inbox synchronization paused for ${filePath}: ${error.message}`);
        }
    }

    async flush() {
        for (const timer of this.timers.values()) clearTimeout(timer);
        this.timers.clear();
        for (const intent of this.pending.values()) intent.ready = true;
        await this.drain();
    }

    async drain() {
        if (this.running) return this.running;
        if (this.disposed) return;
        this.running = this.runQueue().catch((error) => {
            this.notify(`Taskcli synchronization failed: ${error.message}`);
        }).finally(() => {
            this.running = null;
            if (!this.disposed && [...this.pending.values()].some((item) => item.ready)) {
                this.drainTimer = setTimeout(() => void this.drain(), 0);
            }
        });
        return this.running;
    }

    async runQueue() {
        while (!this.disposed) {
            const intent = [...this.pending.values()].find((item) => item.ready);
            if (!intent) break;
            // A write can re-project other queued files. Learn their baselines
            // first so projection echoes cannot replace uncached user edits.
            for (const queued of [...this.pending.values()]) {
                if (queued === intent || queued.status !== undefined) continue;
                try {
                    const note = await this.lookup(queued);
                    if (note && this.pending.get(queued.path) === queued) {
                        queued.status = note.status;
                        queued.properties = note.properties;
                    }
                } catch {
                    // Its own operation will report a read failure and retry.
                }
            }
            if (this.pending.get(intent.path) !== intent || this.disposed) continue;
            this.pending.delete(intent.path);
            this.inFlight = intent;
            await this.apply(intent);
            this.inFlight = null;
            if (!this.pending.has(intent.path)) this.generations.delete(intent.path);
        }
    }

    async apply(intent) {
        let current = this.notes.get(intent.path);
        let committed = false;
        try {
            current = await this.lookup(intent);
            if (this.disposed || this.generations.get(intent.path) !== intent.generation) return;
            if (!current) { this.notes.delete(intent.path); return; }
            intent.status ??= current.status;
            if (!intent.reconcileOnly && current.status !== intent.target) {
                if (current.revision !== intent.revision) throw new Error("The taskcli revision changed. Review the latest state before retrying.");
                const args = commandFor(current, intent.target);
                args.push("--expect-revision", String(intent.revision), "--idempotency-key", intent.key);
                const result = await this.io.execute(args);
                if (this.disposed) return;
                const entity = result.result;
                if (entity?.id !== intent.id || !Number.isInteger(entity.revision) || entity.status !== intent.target) {
                    throw new Error("Invalid taskcli mutation result");
                }
                committed = true;
                // A committed mutation remains authoritative even if subsequent
                // projection or snapshot reads fail. Never restore its old state.
                const properties = { status: entity.status, revision: entity.revision };
                if (Object.hasOwn(entity, "phase")) properties.phase = entity.phase;
                for (const key of ["started_at", "pending_review_at", "completed_at", "cancelled_at", "updated_at"]) {
                    if (Object.hasOwn(entity, key)) {
                        properties[key] = entity[key] === null ? null : localTimestamp(entity[key]);
                    }
                }
                current = { ...current, status: entity.status, revision: entity.revision, properties };
                this.remember(current);
                if (result.projection_pending) {
                    try { await this.io.execute(["sync", "--pending"]); }
                    catch (error) { this.notify(`State saved; document synchronization is pending: ${error.message}`); }
                }
            }
            if (committed) current = await this.lookup(intent);
            if (current?.id === intent.id) {
                const next = this.pending.get(intent.path);
                // A user's second edit may precede our first projection. Only
                // rebase across our own successful write, never external writes.
                if (committed && next?.revision === intent.revision) {
                    next.revision = current.revision;
                    next.status = current.status;
                    next.ready = true;
                }
                await this.restore(current, intent);
            }
        } catch (error) {
            if (this.disposed) return;
            let confirmed = false;
            try {
                current = await this.lookup(intent);
                confirmed = true;
            } catch {
                // Preserve the last confirmed display when the CLI cannot run.
            }
            if (this.disposed) return;
            if (current?.id === intent.id) await this.restore(current, intent);
            if (committed && !confirmed) {
                this.notify(`State saved; the latest projection could not be read: ${error.message}`);
            } else if (!confirmed || current?.status !== intent.target) {
                this.notify(`Could not change ${intent.kind} to ${String(intent.target)}: ${error.message}${confirmed ? "" : " The result could not be confirmed; the last known state is shown."}`);
            }
        }
    }

    async restore(note, intent) {
        if (this.disposed || (intent && this.generations.get(note.path) !== intent.generation)) return;
        const file = await this.io.read(note.filePath || note.path, note);
        if (!file || file.id !== note.id || file.revision > note.revision) return;
        if (intent && ![intent.target, intent.status, note.status].some((status) => equal(status, file.status))) {
            this.observe(note.path, file);
            return;
        }
        const properties = { ...note.properties, status: note.status, revision: note.revision };
        if (Object.entries(properties).every(([key, value]) => equal(file[key], value))) return;
        if (!this.disposed) {
            try {
                await this.io.patch(note.filePath || note.path, { id: file.id, revision: file.revision, status: file.status }, properties, note);
            } catch (error) {
                this.notify(`Could not restore ${note.path}: ${error.message}`);
            }
        }
    }

    forget(filePath) {
        for (const note of [...this.notes.values(), ...this.pending.values(), ...(this.inFlight ? [this.inFlight] : [])]) {
            if (note.filePath === filePath) this.forget(note.path);
        }
        this.pending.delete(filePath);
        this.notes.delete(filePath);
        clearTimeout(this.timers.get(filePath));
        this.timers.delete(filePath);
        if (this.inFlight?.path === filePath) this.generations.set(filePath, (this.generations.get(filePath) || 0) + 1);
        else this.generations.delete(filePath);
    }

    notify(message) {
        if (!this.disposed) this.io.notice(message);
    }

    dispose() {
        this.disposed = true;
        for (const timer of this.timers.values()) clearTimeout(timer);
        clearTimeout(this.drainTimer);
        this.pending.clear();
        this.timers.clear();
        this.notes.clear();
        this.generations.clear();
    }
}

function runCli(settings, args, children = new Set()) {
    return new Promise((resolve, reject) => {
        const argv = ["--json", "--actor", "user:obsidian"];
        if (settings.configPath) argv.push("--config", settings.configPath);
        argv.push(...args);
        const child = execFile(settings.cliPath || "taskcli", argv, {
            cwd: settings.vaultPath, timeout: 30000, maxBuffer: 8 * 1024 * 1024,
            encoding: "utf8", windowsHide: true,
        }, (error, stdout, stderr) => {
            children.delete(child);
            let response;
            try { response = JSON.parse(stdout); }
            catch { reject(new Error(error?.message || stderr || "Invalid taskcli JSON response")); return; }
            if (response.schema_version !== 1 || response.ok !== true) {
                reject(new Error(response.error?.message || error?.message || "Invalid taskcli response"));
                return;
            }
            resolve(response);
        });
        children.add(child);
    });
}

class TaskcliSyncPlugin extends Plugin {
    async onload() {
        this.settings = { cliPath: "taskcli", configPath: "", ...await this.loadData() };
        this.children = new Set();
        this.registerBasesView("taskcliRecentJobs", {
            name: "Recent Jobs", icon: "columns-3",
            factory: (controller, container) => recentJobsView(this.app, controller, container),
            options: () => this.app.internalPlugins?.plugins?.bases?.instance?.registrations?.tasknotesKanban?.options?.() || [],
        });
        this.addSettingTab(new TaskcliSettings(this.app, this));
        this.addCommand({
            id: "refresh", name: "Check connection and refresh state",
            callback: () => this.checkConnection(),
        });
        this.registerEvent(this.app.metadataCache.on("changed", (file, data, cache) => {
            if (!this.engine?.watches(file.path)) return;
            this.engine.observe(file.path, cache.frontmatter);
            this.engine.observeInbox(file.path, data);
        }));
        this.registerEvent(this.app.vault.on("delete", (file) => {
            if (this.engine?.watches(file.path)) this.engine.forget(file.path);
        }));
        this.registerEvent(this.app.vault.on("rename", (file, oldPath) => {
            if (!this.engine?.watches(file.path) && !this.engine?.watches(oldPath)) return;
            this.engine.forget(oldPath);
            return this.inspectFile(file);
        }));
        this.registerEvent(this.app.workspace.on("file-open", (file) => { void this.inspectFile(file); }));
        this.app.workspace.onLayoutReady(() => { if (!this.stopped) void this.connect(); });
    }

    async inspectFile(file) {
        const engine = this.engine;
        if (!(file instanceof TFile) || !engine?.watches(file.path)) return;
        try {
            const source = await this.app.vault.read(file);
            const match = /^(?:\uFEFF)?---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/.exec(source);
            engine.observe(file.path, match ? parseYaml(match[1]) : null, true);
            engine.observeInbox(file.path, source, true);
        } catch (error) {
            engine.notify(`Could not check ${file.path}: ${error.message}`);
        }
    }

    async connect() {
        this.engine?.dispose();
        for (const child of this.children) child.kill();
        const settings = { ...this.settings, vaultPath: this.app.vault.adapter.getBasePath() };
        const execute = (args) => runCli(settings, args, this.children);
        const read = async (filePath, note) => {
            const file = this.app.vault.getAbstractFileByPath(filePath);
            if (!(file instanceof TFile)) return null;
            const content = await this.app.vault.read(file);
            if (note.kind === "inbox") return parseInbox(content, note.project_id).find((entry) => entry.id === note.id);
            const match = /^(?:\uFEFF)?---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/.exec(content);
            return match ? parseYaml(match[1]) : null;
        };
        const engine = new SyncEngine({
            execute, read,
            connection: async () => {
                const { result } = await execute(["obsidian", "connection"]);
                if (result?.protocol_version !== 1) throw new Error("Update taskcli to support on-demand Obsidian queries.");
                if (result?.documents?.format !== "obsidian" ||
                    realpathSync(result.documents.root) !== realpathSync(settings.vaultPath)) {
                    throw new Error("taskcli must be configured for this Obsidian vault.");
                }
                return result;
            },
            lookup: async (id) => {
                const { result } = await execute(["obsidian", "show", id]);
                if (result) {
                    const relative = path.relative(settings.vaultPath, path.resolve(settings.vaultPath, result.path));
                    if (relative === ".." || relative.startsWith("../") || path.isAbsolute(relative)) throw new Error("Note path escapes this vault.");
                }
                return result;
            },
            patch: async (filePath, expected, properties, note) => {
                const file = this.app.vault.getAbstractFileByPath(filePath);
                if (!(file instanceof TFile) || engine.disposed) return false;
                if (note.kind === "inbox") {
                    await this.app.vault.process(file, (source) => engine.disposed ? source : patchInbox(source, note, expected, properties));
                    return true;
                }
                let updated = false;
                await this.app.fileManager.processFrontMatter(file, (frontmatter) => {
                    if (engine.disposed || frontmatter.id !== expected.id ||
                        frontmatter.revision !== expected.revision || !equal(frontmatter.status, expected.status)) return;
                    Object.assign(frontmatter, properties);
                    for (const [old, canonical] of [
                        ["dateCreated", "created_at"], ["created", "created_at"],
                        ["dateModified", "updated_at"], ["updated", "updated_at"],
                        ["completedDate", "completed_at"],
                    ]) {
                        if (Object.hasOwn(frontmatter, old)) {
                            if (!Object.hasOwn(frontmatter, canonical)) frontmatter[canonical] = frontmatter[old];
                            delete frontmatter[old];
                        }
                    }
                    updated = true;
                });
                return updated;
            },
            notice: (message) => new Notice(message, 10000),
        });
        this.engine = engine;
        try {
            await engine.initialize();
            for (const leaf of this.app.workspace.getLeavesOfType("markdown")) {
                await this.inspectFile(leaf.view.file);
            }
            await engine.flush();
        } catch (error) {
            engine.ready = false;
            engine.notify(`Taskcli sync is paused: ${error.message}`);
        }
        return engine;
    }

    async checkConnection() {
        if (this.checkingConnection) return;
        this.checkingConnection = true;
        const progress = new Notice("Checking taskcli connection...", 0);
        try {
            const engine = await this.connect();
            if (engine.ready && !engine.disposed && !this.stopped) {
                new Notice(`Connected to taskcli. Monitoring ${engine.directory}; records are queried when needed.`, 5000);
            }
        } catch (error) {
            if (!this.stopped) new Notice(`Taskcli sync is paused: ${error.message}`, 10000);
        } finally {
            progress.hide();
            this.checkingConnection = false;
        }
    }

    onunload() {
        this.stopped = true;
        this.engine?.dispose();
        for (const child of this.children || []) child.kill();
    }
}

class TaskcliSettings extends PluginSettingTab {
    constructor(app, plugin) {
        super(app, plugin);
        this.plugin = plugin;
    }
    display() {
        this.containerEl.empty();
        for (const [key, name, description] of [
            ["cliPath", "Taskcli executable", "Absolute path to the taskcli executable."],
            ["configPath", "Taskcli configuration", "Configuration for this vault. Leave blank to use the taskcli default."],
        ]) {
            new Setting(this.containerEl).setName(name).setDesc(description).addText((text) => {
                text.setValue(this.plugin.settings[key]).onChange(async (value) => {
                    this.plugin.settings[key] = value.trim();
                    await this.plugin.saveData(this.plugin.settings);
                });
            });
        }
        new Setting(this.containerEl).setName("Apply settings and check connection").addButton((button) => {
            button.setButtonText("Connect").onClick(async () => {
                button.setDisabled(true).setButtonText("Checking...");
                try {
                    await this.plugin.checkConnection();
                } finally {
                    button.setDisabled(false).setButtonText("Connect");
                }
            });
        });
    }
}

module.exports = TaskcliSyncPlugin;
module.exports.SyncEngine = SyncEngine;
module.exports.commandFor = commandFor;
module.exports.runCli = runCli;
module.exports.parseInbox = parseInbox;
module.exports.patchInbox = patchInbox;
module.exports.recentJobsView = recentJobsView;
