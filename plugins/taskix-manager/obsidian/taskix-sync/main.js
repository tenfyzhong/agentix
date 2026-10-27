"use strict";

const { Plugin, PluginSettingTab, Setting, Notice, TFile, Modal, parseYaml } = require("obsidian");
const { execFile } = require("node:child_process");
const { randomUUID } = require("node:crypto");
const { realpathSync } = require("node:fs");
const path = require("node:path");

const CONTENT_EDIT_DELAY_MS = 5000;
const PENDING_DELIVERIES_KEY = "taskix-sync-pending-deliveries";

function localTimestamp(seconds) {
    const date = new Date(seconds * 1000);
    const offset = -date.getTimezoneOffset();
    const pad = value => String(Math.abs(value)).padStart(2, "0");
    return new Date(date.getTime() + offset * 60000).toISOString().replace(/\.000Z$/, "") +
        `${offset < 0 ? "-" : "+"}${pad(Math.trunc(offset / 60))}:${pad(offset % 60)}`;
}

function commandFor(note, target) {
    const reason = `Status changed in Obsidian: ${note.status} -> ${target}`;
    let command;
    if (note.kind === "task") {
        command = { BLOCKED: "block", WAITING_USER: "wait", FAILED: "fail", CANCELLED: "cancel" }[target];
        if (target === "TODO") {
            command = note.status === "FAILED" ? "retry" : ["DONE", "CANCELLED"].includes(note.status) ? "reopen" : undefined;
        }
        if (target === "DONE") command = "done";
        if (target === "IN_PROGRESS") {
            throw new Error("Use taskix claim, plan and start/done with the owning session; this plugin does not hold task leases.");
        }
    } else if (note.kind === "job") {
        if (target === "CANCELLED") command = "cancel";
        if (note.status === "ACTIVE" && target === "COMPLETED") command = "approve";
        if (note.status === "ACTIVE" && target === "PENDING_REVIEW") command = "submit";
        if (note.status === "PENDING_REVIEW") {
            if (target === "ACTIVE") command = "reject";
            if (target === "COMPLETED") command = "approve";
        }
    } else if (note.kind === "milestone") {
        if (note.status === "PLANNED" && target === "ACTIVE") command = "start";
        if (["PLANNED", "ACTIVE"].includes(note.status)) {
            if (target === "COMPLETED") command = "complete";
            if (target === "CANCELLED") command = "cancel";
        }
        if (["COMPLETED", "CANCELLED"].includes(note.status) && target === "PLANNED") command = "reopen";
    } else if (note.kind === "issue") {
        if (["TODO", "ACTIVE"].includes(note.status) && target === "PENDING_REVIEW") command = "submit";
        if (note.status === "TODO" && target === "ACTIVE") command = "start";
        if (note.status === "PENDING_REVIEW" && target === "COMPLETED") command = "approve";
        if (note.status === "PENDING_REVIEW" && target === "ACTIVE") command = "reject";
        if (["COMPLETED", "CANCELLED"].includes(note.status) && target === "TODO") command = "reopen";
        if (["TODO", "ACTIVE", "PENDING_REVIEW"].includes(note.status) && target === "CANCELLED") command = "cancel";
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
        this.directory = null;
        this.archiveDirectory = null;
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
            throw new Error("Invalid taskix note response");
        }
        if (note.path !== (intent.filePath || intent.path) ||
            (intent.project_id && note.project_id !== intent.project_id)) return null;
        const item = note;
        if (!this.disposed && this.generations.get(intent.path) === intent.generation) this.remember(item);
        return item;
    }

    watches(filePath) {
        if (!this.ready || this.disposed || this.directory === null) return false;
        return [this.directory, this.archiveDirectory].filter(directory => directory !== null).some(directory => {
            const relative = path.posix.relative(directory, filePath);
            return relative !== ".." && !relative.startsWith("../") && !path.posix.isAbsolute(relative);
        });
    }

    async initialize() {
        this.ready = false;
        const { documents } = await this.io.connection();
        if (this.disposed) return;
        this.directory = documents.directory;
        this.archiveDirectory = documents.archive_directory || "Archived Projects";
        this.notes.clear();
        this.ready = true;
        await this.reconcile();
    }

    async reconcile() {
        for await (const file of this.io.openNotes?.() || []) {
            this.observe(file.path, file.properties, true);
        }
        await this.flush();
    }

    observe(filePath, properties, reconcileOnly = false) {
        if (!this.ready || this.disposed || !properties) return;
        const kind = typeof properties.id === "string" && /^(task|job|issue|milestone)_/.exec(properties.id)?.[1];
        if (!kind || !Number.isInteger(properties.revision) ||
            (kind === "task" && properties.task_id !== properties.id)) return;
        const queued = this.pending.get(filePath);
        const note = this.notes.get(filePath) || (queued?.status ? queued : undefined);
        if (note && properties.id !== note.id) return;
        if (reconcileOnly && (this.pending.has(filePath) || this.inFlight?.path === filePath)) return;
        // Task/Job revisions are generated by taskix, not by status edits.
        // A newer projection may already lag behind the database by the time
        // this queue runs. Reconcile it instead of replaying it as user intent.
        // A subsequent status edit at that queued revision is still an edit.
        const projection = note &&
            properties.revision > (queued?.revision ?? note.revision);
        if (this.inFlight?.path === filePath && properties.revision > this.inFlight.revision &&
            equal(properties.status, this.inFlight.target)) return;
        if (!reconcileOnly && note && properties.revision === note.revision &&
            equal(properties.status, note.status) &&
            (kind !== "job" || (equal(properties.issue ?? null, note.properties?.issue ?? null) &&
                equal(properties.issue_id ?? null, note.properties?.issue_id ?? null))) &&
            (kind !== "issue" || (equal(properties.milestone_id ?? null, note.properties?.milestone_id ?? null) &&
                equal(properties.milestone ?? null, note.properties?.milestone ?? null))) &&
            this.inFlight?.path !== filePath) {
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
        if (previous && equal(previous.rawTarget, properties.status) &&
            previous.revision === properties.revision &&
            (kind !== "job" || equal(previous.issueLink, properties.issue ?? null)) &&
            (kind !== "issue" || (equal(previous.milestone, properties.milestone_id ?? null) &&
                equal(previous.milestoneLink, properties.milestone ?? null)))) return;
        const generation = (this.generations.get(filePath) || 0) + 1;
        this.generations.set(filePath, generation);
        this.pending.set(filePath, {
            ...note, kind, id: properties.id, path: filePath,
            project_id: properties.project_id || note?.project_id,
            revision: properties.revision, rawTarget: properties.status,
            issueLink: kind === "job" ? properties.issue ?? null : undefined,
            milestone: kind === "issue" ? properties.milestone_id ?? null : undefined,
            milestoneLink: kind === "issue" ? properties.milestone ?? null : undefined,
            target: typeof properties.status === "string" ? properties.status.trim() : properties.status,
            reconcileOnly: reconcileOnly || Boolean(projection),
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
            this.notify(`Taskix synchronization failed: ${error.message}`);
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
            if (!intent.reconcileOnly && intent.kind === "job" &&
                !equal(current.properties?.issue ?? null, intent.issueLink)) {
                if (current.revision !== intent.revision) throw new Error("The taskix revision changed. Review the latest state before retrying.");
                const issue = await this.resolveIssueLink(current, intent.issueLink);
                if (this.disposed || this.generations.get(intent.path) !== intent.generation) return;
                if (issue !== (current.properties?.issue_id ?? null)) {
                    const result = await this.io.execute(["job", "update", intent.id,
                        ...(issue ? ["--issue", issue] : ["--unlink-issue"]),
                        "--expect-revision", String(intent.revision), "--idempotency-key", `${intent.key}:issue`]);
                    if (this.disposed) return;
                    if (result.result?.id !== intent.id || !Number.isInteger(result.result.revision)) throw new Error("Invalid taskix mutation result");
                    committed = true;
                    intent.revision = result.result.revision;
                    current = await this.lookup(intent);
                }
            }
            if (!intent.reconcileOnly && intent.kind === "issue" &&
                !equal(current.properties?.milestone ?? null, intent.milestoneLink) &&
                equal(current.properties?.milestone_id ?? null, intent.milestone)) {
                intent.milestone = await this.resolveMilestoneLink(current, intent.milestoneLink);
                if (this.disposed || this.generations.get(intent.path) !== intent.generation) return;
            }
            if (!intent.reconcileOnly && intent.kind === "issue" &&
                !equal(current.properties?.milestone_id ?? null, intent.milestone)) {
                if (current.revision !== intent.revision) throw new Error("The taskix revision changed. Review the latest state before retrying.");
                if (intent.milestone !== null && typeof intent.milestone !== "string") throw new Error("Milestone must be an ID or empty.");
                const result = await this.io.execute(["issue", "update", intent.id,
                    ...(intent.milestone ? ["--milestone", intent.milestone] : ["--unlink-milestone"]),
                    "--expect-revision", String(intent.revision), "--idempotency-key", `${intent.key}:milestone`]);
                if (this.disposed) return;
                if (result.result?.id !== intent.id || !Number.isInteger(result.result.revision)) throw new Error("Invalid taskix mutation result");
                committed = true;
                intent.revision = result.result.revision;
                current = await this.lookup(intent);
            }
            if (!intent.reconcileOnly && current.status !== intent.target) {
                if (current.revision !== intent.revision) throw new Error("The taskix revision changed. Review the latest state before retrying.");
                const args = commandFor(current, intent.target);
                args.push("--expect-revision", String(intent.revision), "--idempotency-key", intent.key);
                const result = await this.io.execute(args);
                if (this.disposed) return;
                const entity = result.result;
                if (entity?.id !== intent.id || !Number.isInteger(entity.revision) || entity.status !== intent.target) {
                    throw new Error("Invalid taskix mutation result");
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
            } else if (!confirmed || current?.status !== intent.target ||
                (intent.kind === "job" && !equal(current?.properties?.issue ?? null, intent.issueLink)) ||
                (intent.kind === "issue" && !equal(current?.properties?.milestone ?? null, intent.milestoneLink)) ||
                (intent.kind === "issue" && !equal(current?.properties?.milestone_id ?? null, intent.milestone))) {
                this.notify(`Could not change ${intent.kind} to ${String(intent.target)}: ${error.message}${confirmed ? "" : " The result could not be confirmed; the last known state is shown."}`);
            }
        }
    }

    async restore(note, intent) {
        if (this.disposed || (intent && this.generations.get(note.path) !== intent.generation)) return;
        const file = await this.io.read(note.filePath || note.path, note);
        if (!file || file.id !== note.id || file.revision > note.revision) return;
        if (intent && ![intent.rawTarget, intent.target, intent.status, note.status].some((status) => equal(status, file.status))) {
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

    async resolveIssueLink(note, link) {
        if (link === null || link === "") return null;
        if (typeof link !== "string") throw new Error("Issue must be one note link or empty.");
        const match = /^\[\[([^|\]#]+)(?:\|[^\]]*)?\]\]$/.exec(link.trim());
        if (!match) throw new Error("Issue must link to a registered Issue in this Project.");
        const target = await this.io.resolveLink(match[1], note.path);
        if (!target || !/^issue_/.test(target.id || "")) throw new Error("Issue link is missing or does not identify a registered Issue.");
        const issue = await this.io.lookup(target.id);
        if (!issue || issue.kind !== "issue" || issue.project_id !== note.project_id || issue.path !== target.path) {
            throw new Error("Issue link is copied, unregistered, or belongs to another Project.");
        }
        return issue.id;
    }

    async resolveMilestoneLink(note, link) {
        if (link === null || link === "") return null;
        if (typeof link !== "string") throw new Error("Milestone must be one note link or empty.");
        const match = /^\[\[([^|\]#]+)(?:\|[^\]]*)?\]\]$/.exec(link.trim());
        if (!match) throw new Error("Milestone must link to a registered Milestone in this Project.");
        const normalize = value => value.replace(/\\/g, "/").replace(/\.md$/, "");
        const target = normalize(match[1]);
        const { result } = await this.io.execute(["milestone", "list", "--project", note.project_id]);
        const choices = Array.isArray(result) ? result.filter(item => item.project_id === note.project_id &&
            [normalize(path.posix.join(this.directory, item.document_path)), normalize(item.document_path),
                normalize(path.posix.basename(item.document_path))].includes(target)) : [];
        if (choices.length !== 1) throw new Error("Milestone link is missing, ambiguous, or belongs to another Project.");
        return choices[0].id;
    }

    forget(filePath) {
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
        // Protect existing notes even when the desktop app has no memory environment.
        const memoryDocument = args.length === 3 && args[0] === "memory" && args[1] === "document";
        const child = execFile(settings.cliPath || "taskix", argv, {
            cwd: settings.vaultPath, timeout: 30000, maxBuffer: 8 * 1024 * 1024,
            encoding: "utf8", windowsHide: true,
            ...(memoryDocument ? { env: { ...process.env, TASKIX_MEMORY_ENABLED: "true" } } : {}),
        }, (error, stdout, stderr) => {
            children.delete(child);
            let response;
            try { response = JSON.parse(stdout); }
            catch { reject(new Error(error?.message || stderr || "Invalid taskix JSON response")); return; }
            if (response.schema_version !== 1 || response.ok !== true) {
                reject(new Error(response.error?.message || error?.message || "Invalid taskix response"));
                return;
            }
            resolve(response);
        });
        children.add(child);
    });
}

class TaskixSyncPlugin extends Plugin {
    async onload() {
        this.settings = { cliPath: "taskix", configPath: "", ...await this.loadData() };
        this.children = new Set();
        this.memoryChecks = new Map();
        this.memoryTimers = new Map();
        this.memoryNotices = new Map();
        this.deliveryTimers = new Map();
        this.deliveryBodies = new Map();
        this.deliveryQueue = Promise.resolve();
        const pending = this.app.loadLocalStorage(PENDING_DELIVERIES_KEY);
        this.pendingDeliveries = new Set(Array.isArray(pending) ? pending.filter(value => typeof value === "string") : []);
        this.deliveryVersions = new Map();
        this.mergeNoticeObservers = new Map();
        if (typeof document !== "undefined") this.watchMergeNotices(document);
        this.app.workspace.iterateAllLeaves(leaf => this.watchMergeNotices(leaf.view.containerEl.ownerDocument));
        this.registerEvent(this.app.workspace.on("window-open", (_workspaceWindow, win) => {
            this.watchMergeNotices(win.document);
        }));
        this.registerEvent(this.app.workspace.on("window-close", (_workspaceWindow, win) => {
            this.mergeNoticeObservers.get(win.document)?.disconnect();
            this.mergeNoticeObservers.delete(win.document);
        }));
        this.addSettingTab(new TaskixSettings(this.app, this));
        this.addCommand({
            id: "refresh", name: "Check connection and refresh state",
            callback: () => this.checkConnection(),
        });
        this.registerEvent(this.app.workspace.on("file-menu", (menu, file) => {
            if (Array.isArray(file.children) && ["Issues", "Milestones"].includes(path.posix.basename(file.path))) {
                const parent = path.posix.dirname(file.path);
                const projectBoard = this.app.vault.getAbstractFileByPath(path.posix.join(parent, "Board.md"));
                const properties = projectBoard && this.app.metadataCache.getFileCache(projectBoard)?.frontmatter;
                if (this.engine?.watches(file.path) && properties?.["taskix-generated"] === true &&
                    /^prj_/.test(properties.id) && properties.status === "ACTIVE") {
                    const milestone = path.posix.basename(file.path) === "Milestones";
                    menu.addItem(item => item.setTitle(milestone ? "New Milestone" : "New Issue").setIcon("plus")
                        .onClick(() => milestone ? this.prepareMilestone(properties.id, file.path) : this.prepareIssue(properties.id, file.path)));
                }
                return;
            }
            const board = Array.isArray(file.children)
                ? this.app.vault.getAbstractFileByPath(path.posix.join(file.path, "Board.md")) : file;
            if (!board || path.posix.basename(board.path) !== "Board.md" || !this.engine?.watches(board.path)) return;
            const properties = this.app.metadataCache.getFileCache(board)?.frontmatter;
            if (properties?.["taskix-generated"] !== true || !/^prj_/.test(properties.id) ||
                !["ACTIVE", "ARCHIVED"].includes(properties.status)) return;
            const archive = properties.status === "ACTIVE";
            menu.addItem(item => item.setTitle(archive ? "Archive project" : "Restore project")
                .setIcon(archive ? "archive" : "archive-restore")
                .onClick(() => this.archiveProject(properties.id, archive, board.path)));
        }));
        this.registerEvent(this.app.metadataCache.on("changed", (file, data, cache) => {
            if (!this.engine?.watches(file.path)) return;
            this.engine.observe(file.path, cache.frontmatter);
            this.queueDelivery(file.path, data, cache.frontmatter);
            this.styleBoards();
        }));
        this.registerEvent(this.app.vault.on("modify", (file) => this.queueMemory(file)));
        this.registerEvent(this.app.vault.on("delete", (file) => {
            if (this.engine?.watches(file.path)) this.engine.forget(file.path);
        }));
        this.registerEvent(this.app.vault.on("rename", (file, oldPath) => {
            if (!this.engine?.watches(file.path) && !this.engine?.watches(oldPath)) return;
            const projects = path.posix.join(this.engine.directory, "Projects");
            if (Array.isArray(file.children) && path.posix.dirname(oldPath) === projects &&
                path.posix.dirname(file.path) === projects) return this.renameProjectFolder(oldPath);
            this.engine.forget(oldPath);
            return this.inspectFile(file);
        }));
        this.registerEvent(this.app.workspace.on("file-open", (file) => {
            this.styleBoards();
            void this.inspectFile(file);
        }));
        this.registerEvent(this.app.workspace.on("layout-change", () => this.styleBoards()));
        this.app.workspace.onLayoutReady(() => { if (!this.stopped) void this.connect(); });
    }

    watchMergeNotices(doc) {
        if (!doc?.body || !doc.defaultView?.MutationObserver || this.mergeNoticeObservers.has(doc)) return;
        const filter = notice => {
            const message = notice.querySelector(".notice-message")?.textContent;
            // Match only Obsidian's normal automatic-merge notice, never failures.
            const match = /^“(.+)” has been modified externally, merging changes automatically\.$/s.exec(message || "");
            if (!match || !this.engine?.watches(match[1]) || this.engine.disposed) return;
            const file = this.app.vault.getAbstractFileByPath(match[1]);
            const properties = file && this.app.metadataCache.getFileCache(file)?.frontmatter;
            if (properties?.["taskix-generated"] !== true || !/^(?:prj|job|task|issue|milestone|mem)_/.test(properties.id || "")) return;
            notice.remove();
        };
        const observer = new doc.defaultView.MutationObserver(records => {
            if (this.stopped) return;
            const notices = new Set();
            for (const record of records) {
                const parent = record.target?.nodeType === 1 ? record.target : record.target?.parentElement;
                const current = parent?.closest(".notice");
                if (current) notices.add(current);
                for (const node of record.addedNodes) {
                    if (node.nodeType !== 1) continue;
                    if (node.matches(".notice")) notices.add(node);
                    for (const notice of node.querySelectorAll(".notice")) notices.add(notice);
                }
            }
            for (const notice of notices) filter(notice);
        });
        observer.observe(doc.body, { childList: true, subtree: true, characterData: true });
        this.mergeNoticeObservers.set(doc, observer);
    }

    styleBoards() {
        for (const type of ["markdown", "bases"]) {
            for (const leaf of this.app.workspace.getLeavesOfType(type)) {
                const file = leaf.view.file;
                let managed = false;
                if (file && this.engine?.watches(file.path)) {
                    if (type === "bases") {
                        managed = ["Dashboard.base", "Recent Jobs.base", "Issue.base"].some(name =>
                            file.path === path.posix.join(this.engine.directory, name));
                        if (!managed && ["Issue.base", "Milestone.base"].includes(path.posix.basename(file.path))) {
                            const board = this.app.vault.getAbstractFileByPath(path.posix.join(path.posix.dirname(file.path), "Board.md"));
                            managed = this.app.metadataCache.getFileCache(board)?.frontmatter?.["taskix-generated"] === true;
                        }
                    } else {
                        const properties = this.app.metadataCache.getFileCache(file)?.frontmatter;
                        managed = properties?.["taskix-generated"] === true &&
                            Array.isArray(properties.tags) && properties.tags.some(tag => ["agent/board", "agent/job", "agent/issue", "agent/milestone"].includes(tag));
                    }
                }
                leaf.view.containerEl.classList.toggle("taskix-board", managed);
            }
        }
    }

    isMemoryPath(filePath) {
        if (!this.engine?.watches(filePath)) return false;
        const relative = path.posix.relative(this.engine.directory, filePath);
        return /^(?:[^/]+\/)+Memory\/mem_(?:[0-9]{12}_)?[0-9a-f]{32}\.md$/.test(relative);
    }

    persistPendingDeliveries() {
        // Vault-scoped local storage is synchronous; closing during debounce must not lose work.
        this.app.saveLocalStorage(PENDING_DELIVERIES_KEY, [...this.pendingDeliveries]);
    }

    async recoverDeliveries(engine) {
        for (const filePath of [...this.pendingDeliveries]) {
            if (this.stopped || this.engine !== engine || engine.disposed) return;
            if (engine.watches(filePath)) await this.inspectFile(this.app.vault.getAbstractFileByPath(filePath));
        }
    }

    queueDelivery(filePath, source, properties) {
        if (!/^(issue|milestone)_/.test(properties?.id || "") || properties?.["taskix-generated"] !== true) return;
        this.pendingDeliveries.add(filePath);
        this.persistPendingDeliveries();
        const version = (this.deliveryVersions.get(filePath) || 0) + 1;
        this.deliveryVersions.set(filePath, version);
        clearTimeout(this.deliveryTimers.get(filePath));
        this.deliveryTimers.set(filePath, setTimeout(() => {
            this.deliveryTimers.delete(filePath);
            this.deliveryQueue = this.deliveryQueue.then(() => this.syncDelivery(filePath, source, properties, version));
        }, CONTENT_EDIT_DELAY_MS));
        while (this.deliveryTimers.size > 128) {
            const oldest = this.deliveryTimers.keys().next().value;
            clearTimeout(this.deliveryTimers.get(oldest)); this.deliveryTimers.delete(oldest);
        }
    }

    async syncDelivery(filePath, source, properties, version = this.deliveryVersions.get(filePath)) {
        const engine = this.engine;
        if (!engine?.watches(filePath) || engine.disposed || this.stopped || properties?.["taskix-generated"] !== true) return;
        if (version !== this.deliveryVersions.get(filePath)) return;
        const acknowledge = () => {
            if (this.engine !== engine || engine.disposed || this.stopped || version !== this.deliveryVersions.get(filePath)) return;
            this.pendingDeliveries.delete(filePath);
            this.persistPendingDeliveries();
        };
        try {
            const keys = properties.id.startsWith("issue_") ? ["requirement", "acceptance", "notes"] : ["goal"];
            const bodies = keys.map(key => {
                const start = `<!-- taskix:${key}:start -->`, end = `<!-- taskix:${key}:end -->`;
                const lines = source.split(/\r?\n/);
                const starts = lines.flatMap((line, index) => line === start ? [index] : []);
                const ends = lines.flatMap((line, index) => line === end ? [index] : []);
                if (starts.length !== 1 || ends.length !== 1 || starts[0] >= ends[0]) throw new Error(`Restore ${key} markers before sync.`);
                return lines.slice(starts[0] + 1, ends[0]).join("\n");
            });
            const body = JSON.stringify(properties.id.startsWith("issue_") ? [properties.title, ...bodies] : bodies);
            if (this.deliveryBodies.get(filePath) === body) { acknowledge(); return; }
            await engine.flush();
            if (engine !== this.engine || engine.disposed || this.stopped) return;
            await runCli({ ...this.settings, vaultPath: this.app.vault.adapter.getBasePath() },
                ["obsidian", "sync", properties.id], this.children);
            if (engine === this.engine && !engine.disposed) {
                this.deliveryBodies.delete(filePath);
                this.deliveryBodies.set(filePath, body);
                acknowledge();
                while (this.deliveryBodies.size > 128) this.deliveryBodies.delete(this.deliveryBodies.keys().next().value);
            }
        } catch (error) {
            if (engine === this.engine && !engine.disposed && !this.stopped) new Notice(`Could not synchronize delivery content: ${error.message}`, 10000);
        }
    }

    clearDeliveryTimers() {
        for (const timer of this.deliveryTimers?.values() || []) clearTimeout(timer);
        this.deliveryTimers?.clear();
        this.deliveryBodies?.clear();
    }

    async prepareMilestone(id, folderPath) {
        try {
            const settings = { ...this.settings, vaultPath: this.app.vault.adapter.getBasePath() };
            const { result: project } = await runCli(settings, ["project", "show", id], this.children);
            const directory = project?.document_directory || `Projects/${project?.key}`;
            if (project?.id !== id || project.archived_at != null || !Number.isInteger(project.revision) ||
                typeof directory !== "string" || directory.startsWith("/") || /^[A-Za-z]:/.test(directory) ||
                directory.replace(/\\/g, "/").split("/").includes("..") ||
                folderPath !== path.posix.join(this.engine.directory, directory.replace(/\\/g, "/"), "Milestones")) {
                throw new Error("This Milestones folder does not match a registered active Project.");
            }
            if (!this.stopped) this.openMilestoneDialog(project, folderPath);
        } catch (error) {
            if (!this.stopped) new Notice(`Could not create Milestone: ${error.message}`, 10000);
        }
    }

    async prepareIssue(id, folderPath) {
        try {
            const settings = { ...this.settings, vaultPath: this.app.vault.adapter.getBasePath() };
            const { result: project } = await runCli(settings, ["project", "show", id], this.children);
            const directory = project?.document_directory || `Projects/${project?.key}`;
            if (project?.id !== id || project.archived_at != null || !Number.isInteger(project.revision) ||
                typeof directory !== "string" || directory.startsWith("/") || /^[A-Za-z]:/.test(directory) ||
                directory.replace(/\\/g, "/").split("/").includes("..") ||
                folderPath !== path.posix.join(this.engine.directory, directory.replace(/\\/g, "/"), "Issues")) {
                throw new Error("This Issues folder does not match a registered active Project.");
            }
            const { result: milestones } = await runCli(settings, ["milestone", "list", "--project", id], this.children);
            if (!Array.isArray(milestones)) throw new Error("Invalid Milestone list.");
            if (!this.stopped) this.openIssueDialog(project, folderPath, milestones);
        } catch (error) {
            if (!this.stopped) new Notice(`Could not create Issue: ${error.message}`, 10000);
        }
    }

    openMilestoneDialog(project, folderPath) {
        const plugin = this;
        class MilestoneDialog extends Modal {
            onOpen() {
                this.contentEl.createEl("h2", { text: "New Milestone" });
                const fields = { title: "", goal: "" };
                let key = randomUUID();
                new Setting(this.contentEl).setName("Title").addText(text => text.onChange(value => {
                    if (value !== fields.title) key = randomUUID();
                    fields.title = value;
                }));
                new Setting(this.contentEl).setName("Goal (optional)").addText(text => text.onChange(value => {
                    if (value !== fields.goal) key = randomUUID();
                    fields.goal = value;
                }));
                new Setting(this.contentEl).addButton(button => {
                    button.setButtonText("Create Milestone").setCta().onClick(async () => {
                        if (this.saving || !fields.title.trim()) return;
                        this.saving = true;
                        button.setDisabled(true);
                        try {
                            await plugin.createMilestone(project, folderPath, fields, key);
                            this.close();
                        } catch (error) {
                            new Notice(`Could not create Milestone: ${error.message}`, 10000);
                            this.saving = false;
                            button.setDisabled(false);
                        }
                    });
                });
            }
            onClose() { this.contentEl.empty(); }
        }
        new MilestoneDialog(this.app).open();
    }

    openIssueDialog(project, folderPath, milestones) {
        const plugin = this;
        class IssueDialog extends Modal {
            onOpen() {
                this.contentEl.createEl("h2", { text: "New Issue" });
                const fields = { title: "", milestone: "", issue_type: "" };
                let key = randomUUID();
                new Setting(this.contentEl).setName("Title").addText(text => text.onChange(value => {
                    if (value !== fields.title) key = randomUUID();
                    fields.title = value;
                }));
                new Setting(this.contentEl).setName("Type").addDropdown(dropdown => {
                    dropdown.addOption("", "None");
                    for (const type of ["feature", "bug", "enhancement", "documentation", "question", "refactor"]) dropdown.addOption(type, type);
                    dropdown.onChange(value => {
                        if (value !== fields.issue_type) key = randomUUID();
                        fields.issue_type = value;
                    });
                });
                new Setting(this.contentEl).setName("Milestone").addDropdown(dropdown => {
                    dropdown.addOption("", "None");
                    for (const milestone of milestones) dropdown.addOption(milestone.id, milestone.title);
                    dropdown.onChange(value => {
                        if (value !== fields.milestone) key = randomUUID();
                        fields.milestone = value;
                    });
                });
                new Setting(this.contentEl).addButton(button => {
                    button.setButtonText("Create Issue").setCta().onClick(async () => {
                        if (this.saving || !fields.title.trim()) return;
                        this.saving = true;
                        button.setDisabled(true);
                        try {
                            await plugin.createIssue(project, folderPath, fields, key);
                            this.close();
                        } catch (error) {
                            new Notice(`Could not create Issue: ${error.message}`, 10000);
                            this.saving = false;
                            button.setDisabled(false);
                        }
                    });
                });
            }
            onClose() { this.contentEl.empty(); }
        }
        new IssueDialog(this.app).open();
    }

    async createIssue(project, folderPath, fields, key) {
        return this.createDelivery("Issue", project, folderPath, fields, key);
    }

    async createMilestone(project, folderPath, fields, key) {
        return this.createDelivery("Milestone", project, folderPath, fields, key);
    }

    async createDelivery(kind, project, folderPath, fields, key) {
        if (!fields.title?.trim()) throw new Error(`${kind} title is required.`);
        const settings = { ...this.settings, vaultPath: this.app.vault.adapter.getBasePath() };
        const args = [kind.toLowerCase(), "create", "--project", project.id, "--title", fields.title.trim(), "--idempotency-key", key];
        if (kind === "Issue" && fields.issue_type) args.push("--type", fields.issue_type);
        if (kind === "Issue" && fields.milestone) args.push("--milestone", fields.milestone);
        if (kind === "Milestone" && fields.goal?.trim()) args.push("--goal", fields.goal.trim());
        const response = await runCli(settings, args, this.children);
        if (this.stopped) return response;
        if (response.projection_pending) {
            new Notice(`${kind} created; document publication is pending: ${response.projection_pending}. Run taskix sync after resolving the conflict.`, 10000);
            return response;
        }
        const delivery = response.result;
        const relative = delivery?.document_path?.replace(/\\/g, "/");
        const filePath = relative && path.posix.join(this.engine.directory, relative);
        if (delivery?.project_id !== project.id || typeof relative !== "string" || relative.startsWith("/") ||
            /^[A-Za-z]:/.test(relative) || relative.split("/").includes("..") || path.posix.dirname(filePath) !== folderPath) {
            throw new Error(`${kind} created, but the returned document path does not match this Project. Refresh the Project before opening it.`);
        }
        let file = this.app.vault.getAbstractFileByPath(filePath);
        for (let attempt = 0; !file && attempt < 20 && !this.stopped; attempt++) {
            await new Promise(resolve => setTimeout(resolve, 100));
            file = this.app.vault.getAbstractFileByPath(filePath);
        }
        if (this.stopped) return response;
        if (file) await this.app.workspace.getLeaf(false).openFile(file);
        else new Notice(`${kind} created. Refresh the vault before opening its document.`, 10000);
        return response;
    }

    async archiveProject(id, archive, boardPath) {
        try {
            const settings = { ...this.settings, vaultPath: this.app.vault.adapter.getBasePath() };
            const { result: project } = await runCli(settings, ["project", "show", id], this.children);
            if (project?.id !== id || !Number.isInteger(project.revision) ||
                archive !== (project.archived_at == null)) throw new Error("Project archive state changed; refresh and try again.");
            const directory = project.document_directory || `Projects/${project.key}`;
            const normalized = typeof directory === "string" ? directory.replace(/\\/g, "/") : "";
            const absolute = path.posix.isAbsolute(normalized) || /^[A-Za-z]:\//.test(normalized);
            const relative = absolute ? "" : normalized.startsWith("Archived Projects/")
                ? path.posix.join(this.engine.archiveDirectory || "Archived Projects", normalized.slice("Archived Projects/".length))
                : path.posix.join(this.engine.directory, normalized);
            if (typeof project.key !== "string" || !relative || normalized.split("/").includes("..") ||
                boardPath !== path.posix.join(relative, "Board.md")) {
                throw new Error("This Board does not match the registered Project folder.");
            }
            const response = await runCli(settings, ["project", archive ? "archive" : "unarchive", id,
                "--expect-revision", String(project.revision), "--idempotency-key", randomUUID()], this.children);
            if (this.stopped) return;
            this.engine?.forget(boardPath);
            await this.engine?.reconcile();
            new Notice(response.projection_pending
                ? `Project state saved; document move is pending: ${response.projection_pending}. Run taskix sync after resolving the conflict.`
                : `Project ${archive ? "archived" : "restored"}.`, 5000);
        } catch (error) {
            if (!this.stopped) new Notice(`Could not ${archive ? "archive" : "restore"} project: ${error.message}`, 10000);
        }
    }

    queueMemory(file) {
        if (!(file instanceof TFile) || !this.isMemoryPath(file.path)) return;
        const filePath = file.path;
        const engine = this.engine;
        clearTimeout(this.memoryTimers.get(filePath));
        this.memoryTimers.delete(filePath);
        this.memoryTimers.set(filePath, setTimeout(() => {
            this.memoryTimers.delete(filePath);
            if (this.engine === engine && !engine.disposed && file.path === filePath) void this.checkMemory(file);
        }, CONTENT_EDIT_DELAY_MS));
        while (this.memoryTimers.size > 128) {
            const oldest = this.memoryTimers.keys().next().value;
            clearTimeout(this.memoryTimers.get(oldest));
            this.memoryTimers.delete(oldest);
        }
    }

    memoryNotice(engine, filePath, message) {
        if (engine.disposed || this.engine !== engine) return;
        const now = Date.now();
        const previous = this.memoryNotices.get(filePath);
        if (previous !== undefined && now - previous < 30000) return;
        this.memoryNotices.delete(filePath);
        this.memoryNotices.set(filePath, now);
        while (this.memoryNotices.size > 128) this.memoryNotices.delete(this.memoryNotices.keys().next().value);
        engine.notify(message);
    }

    clearMemoryTimers() {
        for (const timer of this.memoryTimers?.values() || []) clearTimeout(timer);
        this.memoryTimers?.clear();
    }

    async checkMemory(file) {
        const engine = this.engine;
        if (!(file instanceof TFile) || !this.isMemoryPath(file.path)) return;
        const filePath = file.path;
        const existing = this.memoryChecks.get(filePath);
        if (existing) { existing.pending = true; return existing.promise; }
        const state = { pending: true };
        this.memoryChecks.set(filePath, state);
        state.promise = (async () => {
            while (state.pending && !engine.disposed && this.engine === engine) {
                state.pending = false;
                try {
                    const observed = await this.app.vault.read(file);
                    const { result } = await engine.io.execute(["memory", "document", filePath]);
                    if (engine.disposed || this.engine !== engine || file.path !== filePath) return;
                    if (result?.path !== filePath || typeof result.text !== "string") {
                        throw new Error("Invalid authoritative memory document");
                    }
                    if (observed === result.text) continue;
                    let restored = false;
                    await this.app.vault.process(file, current => {
                        if (engine.disposed || this.engine !== engine || file.path !== filePath) return current;
                        if (current !== observed) { state.pending = true; return current; }
                        restored = true;
                        return result.text;
                    });
                    if (restored && !engine.disposed) this.memoryNotice(engine, filePath, "Taskix memory is read-only. Your edit was rejected and the database content was restored.");
                } catch (error) {
                    this.memoryNotice(engine, filePath, `Could not restore read-only memory ${filePath}: ${error.message}`);
                }
            }
        })().finally(() => { if (this.memoryChecks.get(filePath) === state) this.memoryChecks.delete(filePath); });
        return state.promise;
    }

    async renameProjectFolder(oldPath) {
        const engine = this.engine;
        for (const filePath of new Set([...engine.notes.keys(), ...engine.pending.keys(),
            ...engine.timers.keys(), ...(engine.inFlight ? [engine.inFlight.path] : [])])) {
            if (filePath.startsWith(`${oldPath}/`)) engine.forget(filePath);
        }
        try {
            await engine.flush();
            if (engine.disposed) return;
            await engine.io.execute(["sync", "--pending"]);
            if (engine.disposed) return;
            for (const leaf of this.app.workspace.getLeavesOfType("markdown")) {
                await this.inspectFile(leaf.view.file);
            }
        } catch (error) {
            engine.notify(`Could not synchronize Project folder rename: ${error.message}`);
        }
    }

    async inspectFile(file) {
        const engine = this.engine;
        if (!(file instanceof TFile) || !engine?.watches(file.path)) return;
        if (this.isMemoryPath(file.path)) return this.checkMemory(file);
        try {
            const source = await this.app.vault.read(file);
            const match = /^(?:\uFEFF)?---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/.exec(source);
            const properties = match ? parseYaml(match[1]) : null;
            if (this.stopped || this.engine !== engine || engine.disposed) return;
            engine.observe(file.path, properties, true);
            this.queueDelivery(file.path, source, properties);
        } catch (error) {
            engine.notify(`Could not check ${file.path}: ${error.message}`);
        }
    }

    async connect() {
        this.clearDeliveryTimers();
        this.clearMemoryTimers();
        this.engine?.dispose();
        for (const type of ["markdown", "bases"]) {
            for (const leaf of this.app.workspace.getLeavesOfType(type)) leaf.view.containerEl.classList.remove("taskix-board");
        }
        for (const child of this.children) child.kill();
        const settings = { ...this.settings, vaultPath: this.app.vault.adapter.getBasePath() };
        const execute = (args) => runCli(settings, args, this.children);
        const read = async (filePath, note) => {
            const file = this.app.vault.getAbstractFileByPath(filePath);
            if (!(file instanceof TFile)) return null;
            const content = await this.app.vault.read(file);
            const match = /^(?:\uFEFF)?---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/.exec(content);
            return match ? parseYaml(match[1]) : null;
        };
        const engine = new SyncEngine({
            execute, read,
            connection: async () => {
                const { result } = await execute(["obsidian", "connection"]);
                if (result?.protocol_version !== 1) throw new Error("Update taskix to support on-demand Obsidian queries.");
                if (typeof result?.documents?.root !== "string" ||
                    realpathSync(result.documents.root) !== realpathSync(settings.vaultPath)) {
                    throw new Error("taskix must be configured for this Obsidian vault.");
                }
                return result;
            },
            resolveLink: (link, sourcePath) => {
                const file = this.app.metadataCache.getFirstLinkpathDest(link, sourcePath);
                if (!(file instanceof TFile)) return null;
                const properties = this.app.metadataCache.getFileCache(file)?.frontmatter;
                return { path: file.path, id: properties?.id };
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
            await this.recoverDeliveries(engine);
            this.styleBoards();
            for (const leaf of this.app.workspace.getLeavesOfType("markdown")) {
                await this.inspectFile(leaf.view.file);
            }
            await engine.flush();
        } catch (error) {
            engine.ready = false;
            engine.notify(`Taskix sync is paused: ${error.message}`);
        }
        return engine;
    }

    async checkConnection() {
        if (this.checkingConnection) return;
        this.checkingConnection = true;
        const progress = new Notice("Checking taskix connection...", 0);
        try {
            const engine = await this.connect();
            if (engine.ready && !engine.disposed && !this.stopped) {
                new Notice(`Connected to taskix. Monitoring ${engine.directory}; records are queried when needed.`, 5000);
            }
        } catch (error) {
            if (!this.stopped) new Notice(`Taskix sync is paused: ${error.message}`, 10000);
        } finally {
            progress.hide();
            this.checkingConnection = false;
        }
    }

    onunload() {
        this.stopped = true;
        for (const observer of this.mergeNoticeObservers?.values() || []) observer.disconnect();
        this.mergeNoticeObservers?.clear();
        this.clearDeliveryTimers();
        this.clearMemoryTimers();
        this.memoryNotices?.clear();
        this.engine?.dispose();
        for (const type of ["markdown", "bases"]) {
            for (const leaf of this.app.workspace.getLeavesOfType(type)) leaf.view.containerEl.classList.remove("taskix-board");
        }
        for (const child of this.children || []) child.kill();
    }
}

class TaskixSettings extends PluginSettingTab {
    constructor(app, plugin) {
        super(app, plugin);
        this.plugin = plugin;
    }
    display() {
        this.containerEl.empty();
        for (const [key, name, description] of [
            ["cliPath", "Taskix executable", "Absolute path to the taskix executable."],
            ["configPath", "Taskix configuration", "Configuration for this vault. Leave blank to use the taskix default."],
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

module.exports = TaskixSyncPlugin;
module.exports.SyncEngine = SyncEngine;
module.exports.commandFor = commandFor;
module.exports.runCli = runCli;
