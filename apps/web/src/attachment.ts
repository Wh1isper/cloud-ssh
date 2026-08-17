import {
  ATTACHMENT_MAX_DIMENSION,
  ATTACHMENT_MAX_GRID_CELLS,
  ATTACHMENT_MAX_INPUT_BYTES,
  ATTACHMENT_MAX_PANES,
  ATTACHMENT_MAX_PANE_SNAPSHOT_BYTES,
  ATTACHMENT_MAX_TERMINAL_CHUNK_BYTES,
} from "./generated/contracts";

const UTF8_ENCODER = new TextEncoder();

export interface AttachmentSessionSummary {
  session_id: string;
  session_created: number;
  name: string;
  attached_client_count: number;
  window_count: number;
}

export interface AttachmentWindowSummary {
  window_id: string;
  name: string;
  active: boolean;
}

export interface AttachmentWindow {
  window_id: string;
  name: string;
  width: number;
  height: number;
  layout: string;
}

export interface AttachmentPane {
  pane_id: string;
  active: boolean;
  width: number;
  height: number;
  left: number;
  top: number;
  title: string;
  current_command: string;
}

export type AttachmentOperation =
  | "writer.claim"
  | "writer.takeover"
  | "session.refresh"
  | "session.create"
  | "window.select"
  | "pane.select"
  | "pane.input"
  | "client.resize"
  | "workspace.refresh";

export type AttachmentErrorCode =
  | "unauthenticated"
  | "invalid_origin"
  | "machine_unavailable"
  | "stale_selection"
  | "stale_epoch"
  | "writer_required"
  | "writer_busy"
  | "operation_ambiguous"
  | "overloaded"
  | "tmux_missing"
  | "tmux_incompatible"
  | "target_unavailable"
  | "protocol_error";

export type AttachmentFrame =
  | { type: "workspace.phase"; phase: "connecting" | "selecting" | "ready" | "failed" }
  | {
      type: "session.list";
      machine_connection_epoch: string;
      selection_epoch: string;
      tmux_client_version: string;
      tmux_server_version: string | null;
      sessions: Array<AttachmentSessionSummary>;
    }
  | {
      type: "writer.state";
      machine_connection_epoch: string;
      attachment_epoch: string;
      role: "observer" | "writer";
      writer_available: boolean;
    }
  | {
      type: "operation.result";
      request_id: string;
      operation: AttachmentOperation;
      outcome: "succeeded" | "failed" | "ambiguous";
      code: string;
      message: string;
    }
  | {
      type: "workspace.projection";
      machine_connection_epoch: string;
      workspace_epoch: string;
      session_id: string;
      session_created: number;
      windows: Array<AttachmentWindowSummary>;
      window: AttachmentWindow;
      panes: Array<AttachmentPane>;
    }
  | {
      type: "workspace.pane_snapshot";
      workspace_epoch: string;
      pane_id: string;
      chunk_index: number;
      final: boolean;
      data: Uint8Array;
    }
  | { type: "workspace.output"; workspace_epoch: string; pane_id: string; data: Uint8Array }
  | { type: "workspace.error"; code: AttachmentErrorCode; message: string };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasKeys(value: Record<string, unknown>, keys: ReadonlyArray<string>): boolean {
  const expected = [...keys].sort();
  const actual = Object.keys(value).sort();
  return actual.length === expected.length && actual.every((key, index) => key === expected[index]);
}

function isUuid(value: unknown): value is string {
  return (
    typeof value === "string" &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value)
  );
}

function isConnectionEpoch(value: unknown): value is string {
  return typeof value === "string" && /^[1-9][0-9]{0,18}$/.test(value);
}

function isNumericId(value: unknown, prefix: "$" | "@" | "%"): value is string {
  return (
    typeof value === "string" && new RegExp(`^\\${prefix}[0-9]+$`).test(value) && value.length <= 32
  );
}

function isBoundedInteger(value: unknown, minimum: number, maximum: number): value is number {
  return Number.isSafeInteger(value) && Number(value) >= minimum && Number(value) <= maximum;
}

function isSafeText(value: unknown, maximum: number, allowEmpty = true): value is string {
  return (
    typeof value === "string" &&
    (allowEmpty || value.length > 0) &&
    UTF8_ENCODER.encode(value).length <= maximum &&
    [...value].every((character) => {
      const code = character.codePointAt(0) ?? 0;
      return code >= 32 && code !== 127;
    })
  );
}

function decodeBase64Url(value: unknown, maximum: number, allowEmpty: boolean): Uint8Array {
  if (
    typeof value !== "string" ||
    (!allowEmpty && value.length === 0) ||
    value.length > Math.ceil((maximum * 4) / 3) ||
    !/^[A-Za-z0-9_-]*$/.test(value) ||
    value.length % 4 === 1
  ) {
    throw new Error("invalid terminal bytes");
  }
  const standard = value.replaceAll("-", "+").replaceAll("_", "/");
  const decoded = atob(standard + "=".repeat((4 - (standard.length % 4)) % 4));
  const bytes = Uint8Array.from(decoded, (character) => character.charCodeAt(0));
  if (bytes.length > maximum) throw new Error("terminal bytes exceed limit");
  if (encodeBase64Url(bytes) !== value) throw new Error("terminal bytes are not canonical");
  return bytes;
}

export function encodeBase64Url(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

export function validateAttachmentInput(bytes: Uint8Array): void {
  if (bytes.length === 0 || bytes.length > ATTACHMENT_MAX_INPUT_BYTES) {
    throw new Error(`Pane input must contain 1-${ATTACHMENT_MAX_INPUT_BYTES} bytes`);
  }
}

function parseSessionList(input: Record<string, unknown>): AttachmentFrame {
  if (
    !hasKeys(input, [
      "machine_connection_epoch",
      "selection_epoch",
      "sessions",
      "tmux_client_version",
      "tmux_server_version",
      "type",
    ]) ||
    !isConnectionEpoch(input.machine_connection_epoch) ||
    !isUuid(input.selection_epoch) ||
    !isSafeText(input.tmux_client_version, 32) ||
    !(input.tmux_server_version === null || isSafeText(input.tmux_server_version, 32)) ||
    !Array.isArray(input.sessions) ||
    input.sessions.length > 128
  ) {
    throw new Error("invalid session list");
  }
  const identities = new Set<string>();
  const sessions = input.sessions.map((session) => {
    if (
      !isRecord(session) ||
      !hasKeys(session, [
        "attached_client_count",
        "name",
        "session_created",
        "session_id",
        "window_count",
      ]) ||
      !isNumericId(session.session_id, "$") ||
      !isBoundedInteger(session.session_created, 1, Number.MAX_SAFE_INTEGER) ||
      !isSafeText(session.name, 128) ||
      !isBoundedInteger(session.attached_client_count, 0, 10_000) ||
      !isBoundedInteger(session.window_count, 1, 10_000)
    ) {
      throw new Error("invalid session");
    }
    const identity = `${session.session_id}:${session.session_created}`;
    if (identities.has(identity)) throw new Error("duplicate session");
    identities.add(identity);
    return {
      session_id: session.session_id,
      session_created: session.session_created,
      name: session.name,
      attached_client_count: session.attached_client_count,
      window_count: session.window_count,
    };
  });
  if (sessions.length > 0 && input.tmux_server_version === null)
    throw new Error("missing tmux server version");
  return {
    type: "session.list",
    machine_connection_epoch: input.machine_connection_epoch,
    selection_epoch: input.selection_epoch,
    tmux_client_version: input.tmux_client_version,
    tmux_server_version: input.tmux_server_version,
    sessions,
  };
}

function parseProjection(input: Record<string, unknown>): AttachmentFrame {
  if (
    !hasKeys(input, [
      "machine_connection_epoch",
      "panes",
      "session_created",
      "session_id",
      "type",
      "window",
      "windows",
      "workspace_epoch",
    ]) ||
    !isConnectionEpoch(input.machine_connection_epoch) ||
    !isUuid(input.workspace_epoch) ||
    !isNumericId(input.session_id, "$") ||
    !isBoundedInteger(input.session_created, 1, Number.MAX_SAFE_INTEGER) ||
    !Array.isArray(input.windows) ||
    input.windows.length === 0 ||
    input.windows.length > 128 ||
    !isRecord(input.window) ||
    !hasKeys(input.window, ["height", "layout", "name", "width", "window_id"]) ||
    !isNumericId(input.window.window_id, "@") ||
    !isSafeText(input.window.name, 128) ||
    !isBoundedInteger(input.window.width, 1, ATTACHMENT_MAX_DIMENSION) ||
    !isBoundedInteger(input.window.height, 1, ATTACHMENT_MAX_DIMENSION) ||
    input.window.width * input.window.height > ATTACHMENT_MAX_GRID_CELLS ||
    !isSafeText(input.window.layout, 4096, false) ||
    !Array.isArray(input.panes) ||
    input.panes.length === 0 ||
    input.panes.length > ATTACHMENT_MAX_PANES
  ) {
    throw new Error("invalid projection");
  }
  const projectedWindow = input.window;
  const windowIds = new Set<string>();
  let activeWindows = 0;
  const windows = input.windows.map((window) => {
    if (
      !isRecord(window) ||
      !hasKeys(window, ["active", "name", "window_id"]) ||
      !isNumericId(window.window_id, "@") ||
      !isSafeText(window.name, 128) ||
      typeof window.active !== "boolean" ||
      windowIds.has(window.window_id)
    ) {
      throw new Error("invalid window summary");
    }
    windowIds.add(window.window_id);
    if (window.active) activeWindows += 1;
    return { window_id: window.window_id, name: window.name, active: window.active };
  });
  if (
    activeWindows !== 1 ||
    !windows.some((window) => window.active && window.window_id === projectedWindow.window_id)
  ) {
    throw new Error("invalid active window");
  }
  const window: AttachmentWindow = {
    window_id: projectedWindow.window_id as string,
    name: projectedWindow.name as string,
    width: projectedWindow.width as number,
    height: projectedWindow.height as number,
    layout: projectedWindow.layout as string,
  };
  const paneIds = new Set<string>();
  let activePanes = 0;
  const panes = input.panes.map((pane) => {
    if (
      !isRecord(pane) ||
      !hasKeys(pane, [
        "active",
        "current_command",
        "height",
        "left",
        "pane_id",
        "title",
        "top",
        "width",
      ]) ||
      !isNumericId(pane.pane_id, "%") ||
      typeof pane.active !== "boolean" ||
      !isBoundedInteger(pane.width, 1, ATTACHMENT_MAX_DIMENSION) ||
      !isBoundedInteger(pane.height, 1, ATTACHMENT_MAX_DIMENSION) ||
      !isBoundedInteger(pane.left, 0, ATTACHMENT_MAX_DIMENSION - 1) ||
      !isBoundedInteger(pane.top, 0, ATTACHMENT_MAX_DIMENSION - 1) ||
      pane.left + pane.width > window.width ||
      pane.top + pane.height > window.height ||
      !isSafeText(pane.title, 256) ||
      !isSafeText(pane.current_command, 256) ||
      paneIds.has(pane.pane_id)
    ) {
      throw new Error("invalid pane");
    }
    paneIds.add(pane.pane_id);
    if (pane.active) activePanes += 1;
    return {
      pane_id: pane.pane_id,
      active: pane.active,
      width: pane.width,
      height: pane.height,
      left: pane.left,
      top: pane.top,
      title: pane.title,
      current_command: pane.current_command,
    };
  });
  const totalCells = panes.reduce((total, pane) => total + pane.width * pane.height, 0);
  if (activePanes !== 1 || totalCells > ATTACHMENT_MAX_GRID_CELLS)
    throw new Error("invalid active pane cardinality or grid budget");
  return {
    type: "workspace.projection",
    machine_connection_epoch: input.machine_connection_epoch,
    workspace_epoch: input.workspace_epoch,
    session_id: input.session_id,
    session_created: input.session_created,
    windows,
    window,
    panes,
  };
}

export function parseAttachmentFrame(input: unknown): AttachmentFrame {
  if (!isRecord(input) || typeof input.type !== "string") throw new Error("invalid frame");
  switch (input.type) {
    case "workspace.phase":
      if (
        !hasKeys(input, ["phase", "type"]) ||
        !["connecting", "selecting", "ready", "failed"].includes(String(input.phase))
      )
        throw new Error("invalid phase");
      return input as AttachmentFrame;
    case "session.list":
      return parseSessionList(input);
    case "writer.state":
      if (
        !hasKeys(input, [
          "attachment_epoch",
          "machine_connection_epoch",
          "role",
          "type",
          "writer_available",
        ]) ||
        !isConnectionEpoch(input.machine_connection_epoch) ||
        !isUuid(input.attachment_epoch) ||
        !["observer", "writer"].includes(String(input.role)) ||
        typeof input.writer_available !== "boolean"
      )
        throw new Error("invalid writer state");
      return input as AttachmentFrame;
    case "operation.result": {
      const operations: ReadonlyArray<AttachmentOperation> = [
        "writer.claim",
        "writer.takeover",
        "session.refresh",
        "session.create",
        "window.select",
        "pane.select",
        "pane.input",
        "client.resize",
        "workspace.refresh",
      ];
      if (
        !hasKeys(input, ["code", "message", "operation", "outcome", "request_id", "type"]) ||
        !isUuid(input.request_id) ||
        !operations.includes(input.operation as AttachmentOperation) ||
        !["succeeded", "failed", "ambiguous"].includes(String(input.outcome)) ||
        !isSafeText(input.code, 64, false) ||
        !isSafeText(input.message, 256)
      )
        throw new Error("invalid operation result");
      return input as AttachmentFrame;
    }
    case "workspace.projection":
      return parseProjection(input);
    case "workspace.pane_snapshot":
      if (
        !hasKeys(input, [
          "chunk_index",
          "data_base64",
          "final",
          "pane_id",
          "type",
          "workspace_epoch",
        ]) ||
        !isUuid(input.workspace_epoch) ||
        !isNumericId(input.pane_id, "%") ||
        !isBoundedInteger(
          input.chunk_index,
          0,
          ATTACHMENT_MAX_PANE_SNAPSHOT_BYTES / ATTACHMENT_MAX_TERMINAL_CHUNK_BYTES - 1,
        ) ||
        typeof input.final !== "boolean"
      )
        throw new Error("invalid pane snapshot");
      return {
        type: "workspace.pane_snapshot",
        workspace_epoch: input.workspace_epoch,
        pane_id: input.pane_id,
        chunk_index: input.chunk_index,
        final: input.final,
        data: decodeBase64Url(input.data_base64, ATTACHMENT_MAX_TERMINAL_CHUNK_BYTES, true),
      };
    case "workspace.output":
      if (
        !hasKeys(input, ["data_base64", "pane_id", "type", "workspace_epoch"]) ||
        !isUuid(input.workspace_epoch) ||
        !isNumericId(input.pane_id, "%")
      )
        throw new Error("invalid output");
      return {
        type: "workspace.output",
        workspace_epoch: input.workspace_epoch,
        pane_id: input.pane_id,
        data: decodeBase64Url(input.data_base64, ATTACHMENT_MAX_TERMINAL_CHUNK_BYTES, true),
      };
    case "workspace.error": {
      const codes: ReadonlyArray<AttachmentErrorCode> = [
        "unauthenticated",
        "invalid_origin",
        "machine_unavailable",
        "stale_selection",
        "stale_epoch",
        "writer_required",
        "writer_busy",
        "operation_ambiguous",
        "overloaded",
        "tmux_missing",
        "tmux_incompatible",
        "target_unavailable",
        "protocol_error",
      ];
      if (
        !hasKeys(input, ["code", "message", "type"]) ||
        !codes.includes(input.code as AttachmentErrorCode) ||
        !isSafeText(input.message, 256)
      )
        throw new Error("invalid error");
      return {
        type: "workspace.error",
        code: input.code as AttachmentErrorCode,
        message: input.message,
      };
    }
    default:
      throw new Error("unsupported frame");
  }
}
