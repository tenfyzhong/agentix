"use strict";

const { Plugin, PluginSettingTab, Setting, Notice, TFile, parseYaml } = require("obsidian");
const { execFile } = require("node:child_process");
const { randomUUID } = require("node:crypto");
const { realpathSync } = require("node:fs");
const path = require("node:path");

function commandFor(note, target) {
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
        this.refreshRequested = false;
    }

    adopt(snapshot) {
        if (!snapshot || !Array.isArray(snapshot.notes)) throw new Error("Invalid taskcli snapshot");
        this.notes = new Map(snapshot.notes.map((note) => [note.path, note]));
    }

    async initialize() {
        this.adopt(await this.io.snapshot());
        if (this.disposed) return;
        this.ready = true;
        await this.reconcile();
    }

    async reconcile() {
        for (const note of this.notes.values()) {
            if (!this.pending.has(note.path) && this.inFlight?.path !== note.path) {
                await this.restore(note);
            }
        }
    }

    observe(filePath, properties) {
        if (!this.ready || this.disposed || !properties) return;
        const note = this.notes.get(filePath);
        if (!note) {
            if (typeof properties.id === "string" && /^(task|job)_/.test(properties.id)) this.requestRefresh();
            return;
        }
        if (properties.id !== note.id || (note.kind === "task" && properties.task_id !== note.id)) return;
        if (properties.revision !== note.revision) {
            this.requestRefresh();
            return;
        }
        if (this.inFlight?.path === filePath && properties.revision > this.inFlight.revision &&
            equal(properties.status, note.status)) return;
        if (equal(properties.status, note.status) && this.inFlight?.path !== filePath) {
            // One CLI write projects every note. Do not let that projection
            // discard a different note's already queued user edit.
            if (this.inFlight && this.pending.has(filePath)) return;
            this.pending.delete(filePath);
            clearTimeout(this.timers.get(filePath));
            this.timers.delete(filePath);
            return;
        }
        const previous = this.pending.get(filePath);
        if (previous && equal(previous.target, properties.status)) return;
        const generation = (this.generations.get(filePath) || 0) + 1;
        this.generations.set(filePath, generation);
        this.pending.set(filePath, {
            ...note, target: properties.status, generation, key: randomUUID(), ready: false,
        });
        clearTimeout(this.timers.get(filePath));
        this.timers.set(filePath, setTimeout(() => {
            this.timers.delete(filePath);
            const intent = this.pending.get(filePath);
            if (intent) intent.ready = true;
            void this.drain();
        }, 300));
    }

    requestRefresh() {
        this.refreshRequested = true;
        if (!this.running) {
            clearTimeout(this.refreshTimer);
            this.refreshTimer = setTimeout(() => void this.drain(), 300);
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
            if (!this.disposed && (this.refreshRequested || [...this.pending.values()].some((item) => item.ready))) {
                this.drainTimer = setTimeout(() => void this.drain(), 0);
            }
        });
        return this.running;
    }

    async runQueue() {
        while (!this.disposed) {
            const intent = [...this.pending.values()].find((item) => item.ready);
            if (!intent) break;
            this.pending.delete(intent.path);
            this.inFlight = intent;
            await this.apply(intent);
            this.inFlight = null;
        }
        if (this.refreshRequested && !this.disposed) {
            this.refreshRequested = false;
            this.adopt(await this.io.snapshot());
            await this.reconcile();
        }
    }

    async apply(intent) {
        let current = this.notes.get(intent.path);
        let committed = false;
        try {
            this.adopt(await this.io.snapshot());
            if (this.disposed || this.generations.get(intent.path) !== intent.generation) return;
            current = this.notes.get(intent.path);
            if (!current || current.id !== intent.id) throw new Error("The registered note was renamed or removed.");
            if (current.status !== intent.target) {
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
                for (const key of ["started_at", "completed_at", "updated_at"]) {
                    if (Object.hasOwn(entity, key)) {
                        properties[key] = entity[key] === null ? null : new Date(entity[key] * 1000).toISOString();
                    }
                }
                if (Object.hasOwn(properties, "completed_at")) properties.completedDate = properties.completed_at;
                if (Object.hasOwn(properties, "updated_at")) properties.dateModified = properties.updated_at;
                current = { ...current, status: entity.status, revision: entity.revision, properties };
                this.notes.set(intent.path, current);
                if (result.projection_pending) {
                    try { await this.io.execute(["sync"]); }
                    catch (error) { this.notify(`State saved; document synchronization is pending: ${error.message}`); }
                }
            }
            this.adopt(await this.io.snapshot());
            current = this.notes.get(intent.path);
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
                this.adopt(await this.io.snapshot());
                current = this.notes.get(intent.path);
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
        const file = await this.io.read(note.path);
        if (!file || file.id !== note.id || file.revision > note.revision) return;
        if (intent && ![intent.target, intent.status, note.status].some((status) => equal(status, file.status))) {
            this.observe(note.path, file);
            return;
        }
        const properties = { ...note.properties, status: note.status, revision: note.revision };
        if (Object.entries(properties).every(([key, value]) => equal(file[key], value))) return;
        if (!this.disposed) {
            try {
                await this.io.patch(note.path, { id: file.id, revision: file.revision, status: file.status }, properties);
            } catch (error) {
                this.notify(`Could not restore ${note.path}: ${error.message}`);
            }
        }
    }

    forget(filePath) {
        this.pending.delete(filePath);
        this.notes.delete(filePath);
        clearTimeout(this.timers.get(filePath));
        this.timers.delete(filePath);
        this.generations.set(filePath, (this.generations.get(filePath) || 0) + 1);
    }

    notify(message) {
        if (!this.disposed) this.io.notice(message);
    }

    dispose() {
        this.disposed = true;
        for (const timer of this.timers.values()) clearTimeout(timer);
        clearTimeout(this.refreshTimer);
        clearTimeout(this.drainTimer);
        this.pending.clear();
        this.timers.clear();
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
        this.addSettingTab(new TaskcliSettings(this.app, this));
        this.addCommand({
            id: "refresh", name: "Check connection and refresh state",
            callback: () => this.checkConnection(),
        });
        this.registerEvent(this.app.metadataCache.on("changed", (file, _data, cache) => {
            this.engine?.observe(file.path, cache.frontmatter);
        }));
        this.registerEvent(this.app.vault.on("delete", (file) => this.engine?.forget(file.path)));
        this.registerEvent(this.app.vault.on("rename", (file, oldPath) => {
            this.engine?.forget(oldPath);
            this.engine?.requestRefresh();
        }));
        this.app.workspace.onLayoutReady(() => { if (!this.stopped) void this.connect(); });
    }

    async connect() {
        this.engine?.dispose();
        for (const child of this.children) child.kill();
        const settings = { ...this.settings, vaultPath: this.app.vault.adapter.getBasePath() };
        const execute = (args) => runCli(settings, args, this.children);
        const read = async (filePath) => {
            const file = this.app.vault.getAbstractFileByPath(filePath);
            if (!(file instanceof TFile)) return null;
            const content = await this.app.vault.read(file);
            const match = /^(?:\uFEFF)?---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/.exec(content);
            return match ? parseYaml(match[1]) : null;
        };
        const engine = new SyncEngine({
            execute, read,
            snapshot: async () => {
                const { result } = await execute(["obsidian", "snapshot"]);
                if (result?.documents?.format !== "obsidian" ||
                    realpathSync(result.documents.root) !== realpathSync(settings.vaultPath)) {
                    throw new Error("taskcli must be configured for this Obsidian vault.");
                }
                for (const note of result.notes || []) {
                    const relative = path.relative(settings.vaultPath, path.resolve(settings.vaultPath, note.path));
                    if (relative.startsWith("..") || path.isAbsolute(relative)) throw new Error("Snapshot path escapes this vault.");
                }
                return result;
            },
            patch: async (filePath, expected, properties) => {
                const file = this.app.vault.getAbstractFileByPath(filePath);
                if (!(file instanceof TFile) || engine.disposed) return false;
                let updated = false;
                await this.app.fileManager.processFrontMatter(file, (frontmatter) => {
                    if (engine.disposed || frontmatter.id !== expected.id ||
                        frontmatter.revision !== expected.revision || !equal(frontmatter.status, expected.status)) return;
                    Object.assign(frontmatter, properties);
                    updated = true;
                });
                return updated;
            },
            notice: (message) => new Notice(message, 10000),
        });
        this.engine = engine;
        try {
            await engine.initialize();
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
                new Notice(`Connected to taskcli. Monitoring ${engine.notes.size} notes.`, 5000);
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
