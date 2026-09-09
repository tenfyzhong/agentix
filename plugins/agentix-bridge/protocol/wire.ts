// Generated from schema.json. Do not edit.
export type SessionStatus = "notLoaded" | "idle" | "active" | "systemError" | "offline" | "unknown";
export type TurnStatus = "inProgress" | "completed" | "interrupted" | "failed" | "unknown";
export type ErrorCode = "invalid_request" | "unsupported_method" | "unsupported_command" | "busy" | "delivery_uncertain" | "host_error" | "frame_too_large" | "session_changed";
export interface Session {
    id: string;
    name: string | null;
    preview: string | null;
    cwd: string | null;
    updatedAt: number | null;
    status: SessionStatus;
}
export interface Tool {
    kind: string;
    label: string;
    status: string;
}
export interface Item {
    id: string;
    kind: string;
    text: string | null;
    status: string | null;
}
export interface Turn {
    id: string;
    status: TurnStatus;
    user_text: string | null;
    agent_text: string | null;
    tools: Array<Tool>;
    items: Array<Item>;
}
export interface QueueItem {
    id: string;
    text: string;
}
export interface QueueState {
    items: Array<QueueItem>;
    paused: boolean;
    uncertain: Record<string, unknown> | null;
}
export interface Snapshot {
    instance: string;
    seq: number;
    session: Session;
    capabilities: Array<string>;
    turns: Array<Turn>;
    queue: QueueState;
}
export interface Registration {
    version: number;
    agent: string;
    instance: string;
    pid: number;
    session_id: string;
    cwd: string;
    session_file: string;
    snapshot: SessionInfo;
}
export interface History {
    turns: Array<Turn>;
    older_cursor: string | null;
    newer_cursor: string | null;
}
export interface Prompt {
    text: string;
    request_id: string;
}
export interface PromptResult {
    turn_id: string;
}
export interface Choice {
    label: string;
    value: string;
}
export interface CommandResult {
    body: string;
    choices: Array<Choice>;
}
export interface Request {
    id: string;
    method: string;
    params: Record<string, unknown>;
}
export interface Failure {
    id: string;
    ok: boolean;
    error: string;
    code: ErrorCode;
}
export interface SessionExited {
    session_id: string;
}
export interface QueueChanged {
    session_id: string;
}
export interface TurnStarted {
    session_id: string;
    turn_id: string;
}
export interface AgentMessageDelta {
    session_id: string;
    turn_id: string;
    item_id: string;
    delta: string;
}
export interface ItemStarted {
    session_id: string;
    turn_id: string;
    item_id: string;
    kind: string;
    label: string;
}
export interface ItemCompleted {
    session_id: string;
    turn_id: string;
    item: Item;
}
export interface TurnCompleted {
    session_id: string;
    turn_id: string;
    status: TurnStatus;
    error: string | null;
}
export type Event = { SessionExited: SessionExited } | { QueueChanged: QueueChanged } | { TurnStarted: TurnStarted } | { AgentMessageDelta: AgentMessageDelta } | { ItemStarted: ItemStarted } | { ItemCompleted: ItemCompleted } | { TurnCompleted: TurnCompleted };
export interface EventFrame {
    instance: string;
    seq: number;
    event: Event;
}
export interface SessionInfo {
    instance: string;
    seq: number;
    session: Session;
    capabilities: Array<string>;
}
