import {
  ATTACHMENT_MAX_FRAME_BYTES,
  ATTACHMENT_MAX_PANES,
  ATTACHMENT_MAX_PANE_SNAPSHOT_BYTES,
  ATTACHMENT_MAX_PROJECTION_BYTES,
  ATTACHMENT_MAX_TERMINAL_CHUNK_BYTES,
} from "./generated/contracts";
import type {
  CreateCredentialInput,
  CreateMachineInput,
  CredentialSummary,
  DeploymentPresentation,
  EnrollmentTokenResponse,
  MachineCreated,
  MachineSummary,
} from "./generated/contracts";

export class AuthenticationError extends Error {}

const UTF8_ENCODER = new TextEncoder();

export interface AttachmentSession {
  detach(): void;
  dispose(): void;
  returnToChooser(): void;
  selectSession(selectionEpoch: string, sessionId: string, sessionCreated: number): void;
}

export interface AttachmentSessionSummary {
  session_id: string;
  session_created: number;
  name: string;
  attached_client_count: number;
  window_count: number;
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

export type AttachmentFrame =
  | { type: "workspace.phase"; phase: "connecting" | "selecting" | "ready" | "failed" }
  | {
      type: "session.list";
      selection_epoch: string;
      tmux_client_version: string;
      tmux_server_version: string | null;
      sessions: Array<AttachmentSessionSummary>;
    }
  | {
      type: "workspace.projection";
      workspace_epoch: string;
      session_id: string;
      session_created: number;
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

type AttachmentErrorCode =
  | "unauthenticated"
  | "invalid_origin"
  | "machine_unavailable"
  | "stale_selection"
  | "tmux_missing"
  | "tmux_incompatible"
  | "target_unavailable"
  | "protocol_error";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasKeys(value: Record<string, unknown>, keys: ReadonlyArray<string>): boolean {
  const actual = Object.keys(value).sort();
  return (
    actual.length === keys.length && actual.every((key, index) => key === [...keys].sort()[index])
  );
}

function isUuid(value: unknown): value is string {
  return (
    typeof value === "string" &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value)
  );
}

function isNumericId(value: unknown, prefix: "$" | "@" | "%"): value is string {
  return (
    typeof value === "string" && new RegExp(`^\\${prefix}[0-9]+$`).test(value) && value.length <= 32
  );
}

function isBoundedInteger(value: unknown, minimum: number, maximum: number): value is number {
  return Number.isSafeInteger(value) && Number(value) >= minimum && Number(value) <= maximum;
}

function isSafeText(value: unknown, maximum: number): value is string {
  return (
    typeof value === "string" &&
    UTF8_ENCODER.encode(value).length <= maximum &&
    [...value].every((character) => {
      const code = character.codePointAt(0) ?? 0;
      return code >= 32 && code !== 127;
    })
  );
}

function decodeBase64Url(value: unknown): Uint8Array {
  if (
    typeof value !== "string" ||
    value.length > Math.ceil((ATTACHMENT_MAX_TERMINAL_CHUNK_BYTES * 4) / 3) ||
    !/^[A-Za-z0-9_-]*$/.test(value) ||
    value.length % 4 === 1
  ) {
    throw new Error("invalid terminal bytes");
  }
  const standard = value.replaceAll("-", "+").replaceAll("_", "/");
  const decoded = atob(standard + "=".repeat((4 - (standard.length % 4)) % 4));
  const bytes = Uint8Array.from(decoded, (character) => character.charCodeAt(0));
  if (bytes.length > ATTACHMENT_MAX_TERMINAL_CHUNK_BYTES)
    throw new Error("terminal bytes exceed limit");
  const canonical = btoa(String.fromCharCode(...bytes))
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/=+$/, "");
  if (canonical !== value) throw new Error("terminal bytes are not canonical");
  return bytes;
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
    case "session.list": {
      if (
        !hasKeys(input, [
          "selection_epoch",
          "sessions",
          "tmux_client_version",
          "tmux_server_version",
          "type",
        ]) ||
        !isUuid(input.selection_epoch) ||
        !isSafeText(input.tmux_client_version, 32) ||
        !(input.tmux_server_version === null || isSafeText(input.tmux_server_version, 32)) ||
        !Array.isArray(input.sessions) ||
        input.sessions.length > 128
      )
        throw new Error("invalid session list");
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
        )
          throw new Error("invalid session");
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
        selection_epoch: input.selection_epoch,
        tmux_client_version: input.tmux_client_version,
        tmux_server_version: input.tmux_server_version,
        sessions,
      };
    }
    case "workspace.projection": {
      if (
        !hasKeys(input, [
          "panes",
          "session_created",
          "session_id",
          "type",
          "window",
          "workspace_epoch",
        ]) ||
        !isUuid(input.workspace_epoch) ||
        !isNumericId(input.session_id, "$") ||
        !isBoundedInteger(input.session_created, 1, Number.MAX_SAFE_INTEGER) ||
        !isRecord(input.window) ||
        !hasKeys(input.window, ["height", "layout", "name", "width", "window_id"]) ||
        !isNumericId(input.window.window_id, "@") ||
        !isSafeText(input.window.name, 128) ||
        !isBoundedInteger(input.window.width, 1, 10_000) ||
        !isBoundedInteger(input.window.height, 1, 10_000) ||
        !isSafeText(input.window.layout, 4096) ||
        input.window.layout.length === 0 ||
        !Array.isArray(input.panes) ||
        input.panes.length === 0 ||
        input.panes.length > ATTACHMENT_MAX_PANES
      )
        throw new Error("invalid projection");
      const window = {
        window_id: input.window.window_id,
        name: input.window.name,
        width: input.window.width,
        height: input.window.height,
        layout: input.window.layout,
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
          !isBoundedInteger(pane.width, 1, 10_000) ||
          !isBoundedInteger(pane.height, 1, 10_000) ||
          !isBoundedInteger(pane.left, 0, 9999) ||
          !isBoundedInteger(pane.top, 0, 9999) ||
          pane.left + pane.width > window.width ||
          pane.top + pane.height > window.height ||
          !isSafeText(pane.title, 256) ||
          !isSafeText(pane.current_command, 256) ||
          paneIds.has(pane.pane_id)
        )
          throw new Error("invalid pane");
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
      if (activePanes !== 1) throw new Error("invalid active pane cardinality");
      return {
        type: "workspace.projection",
        workspace_epoch: input.workspace_epoch,
        session_id: input.session_id,
        session_created: input.session_created,
        window,
        panes,
      };
    }
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
        data: decodeBase64Url(input.data_base64),
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
        data: decodeBase64Url(input.data_base64),
      };
    case "workspace.error": {
      const codes: ReadonlyArray<AttachmentErrorCode> = [
        "unauthenticated",
        "invalid_origin",
        "machine_unavailable",
        "stale_selection",
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

export interface ApiClient {
  cancelEnrollment(machineId: string): Promise<void>;
  createCredential(input: CreateCredentialInput): Promise<CredentialSummary>;
  createMachine(input: CreateMachineInput): Promise<MachineCreated>;
  deployment(): Promise<DeploymentPresentation>;
  disableMachine(machineId: string): Promise<void>;
  dispose(): void;
  enableMachine(machineId: string): Promise<void>;
  issueEnrollment(machineId: string): Promise<EnrollmentTokenResponse>;
  reEnrollMachine(machineId: string): Promise<void>;
  openAttachment(
    machineId: string,
    onFrame: (frame: AttachmentFrame) => void,
    onClose: () => void,
  ): AttachmentSession;
  listCredentials(): Promise<Array<CredentialSummary>>;
  listMachines(): Promise<Array<MachineSummary>>;
  renameCredential(credentialId: string, name: string): Promise<CredentialSummary>;
  resetDefaultCredential(name: string): Promise<CredentialSummary>;
  retireCredential(credentialId: string): Promise<void>;
  setDefaultCredential(credentialId: string): Promise<void>;
}

export function createApiClient(candidate: string): ApiClient {
  let apiKey = candidate;
  const controllers = new Set<AbortController>();
  const sockets = new Set<WebSocket>();
  let disposed = false;

  async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
    if (disposed || apiKey.length === 0) {
      throw new AuthenticationError("API client is disposed");
    }
    const url = new URL(path, window.location.origin);
    if (url.origin !== window.location.origin || !url.pathname.startsWith("/api/v1/")) {
      throw new Error("API request must remain on the Deployment origin");
    }
    const controller = new AbortController();
    controllers.add(controller);
    const headers = new Headers(init.headers);
    headers.delete("Authorization");
    headers.set("Authorization", `Bearer ${apiKey}`);
    if (init.body !== undefined) {
      headers.set("Content-Type", "application/json");
    }
    try {
      const response = await fetch(url, {
        ...init,
        cache: "no-store",
        credentials: "omit",
        headers,
        signal: controller.signal,
      });
      if (response.status === 401) {
        dispose();
        throw new AuthenticationError("Authentication failed");
      }
      if (!response.ok) {
        const body = (await response.json().catch(() => null)) as { message?: string } | null;
        throw new Error(body?.message ?? `Request failed (${response.status})`);
      }
      if (response.status === 204) {
        return undefined as T;
      }
      return (await response.json()) as T;
    } finally {
      controllers.delete(controller);
    }
  }

  function dispose(): void {
    if (disposed) return;
    disposed = true;
    apiKey = "";
    for (const controller of controllers) controller.abort();
    controllers.clear();
    for (const socket of sockets) socket.close(1000, "logout");
    sockets.clear();
  }

  function openAttachment(
    machineId: string,
    onFrame: (frame: AttachmentFrame) => void,
    onClose: () => void,
  ): AttachmentSession {
    if (disposed || apiKey.length === 0) throw new AuthenticationError("API client is disposed");
    const url = new URL(
      `/attachment/v1/machines/${encodeURIComponent(machineId)}`,
      window.location.origin,
    );
    url.protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(url);
    sockets.add(socket);
    let closed = false;
    let pendingProjection: {
      workspaceEpoch: string;
      paneIds: Set<string>;
      nextChunk: Map<string, number>;
      completed: Set<string>;
      paneBytes: Map<string, number>;
      totalBytes: number;
    } | null = null;
    let currentWorkspaceEpoch: string | null = null;
    let currentPaneIds = new Set<string>();

    const validateSequence = (frame: AttachmentFrame): boolean => {
      if (frame.type === "workspace.phase") {
        if (
          frame.phase === "connecting" ||
          frame.phase === "selecting" ||
          frame.phase === "failed"
        ) {
          pendingProjection = null;
          currentWorkspaceEpoch = null;
          currentPaneIds = new Set();
        } else {
          if (
            pendingProjection === null ||
            pendingProjection.completed.size !== pendingProjection.paneIds.size
          )
            throw new Error("incomplete projection");
          currentWorkspaceEpoch = pendingProjection.workspaceEpoch;
          currentPaneIds = new Set(pendingProjection.paneIds);
          pendingProjection = null;
        }
        return true;
      }
      if (frame.type === "session.list") {
        pendingProjection = null;
        currentWorkspaceEpoch = null;
        currentPaneIds = new Set();
        return true;
      }
      if (frame.type === "workspace.projection") {
        const paneIds = new Set(frame.panes.map((pane) => pane.pane_id));
        pendingProjection = {
          workspaceEpoch: frame.workspace_epoch,
          paneIds,
          nextChunk: new Map([...paneIds].map((paneId) => [paneId, 0])),
          completed: new Set(),
          paneBytes: new Map([...paneIds].map((paneId) => [paneId, 0])),
          totalBytes: 0,
        };
        currentWorkspaceEpoch = null;
        currentPaneIds = new Set();
        return true;
      }
      if (frame.type === "workspace.pane_snapshot") {
        if (
          pendingProjection === null ||
          frame.workspace_epoch !== pendingProjection.workspaceEpoch ||
          !pendingProjection.paneIds.has(frame.pane_id) ||
          pendingProjection.completed.has(frame.pane_id) ||
          frame.chunk_index !== pendingProjection.nextChunk.get(frame.pane_id) ||
          (!frame.final && frame.data.length === 0)
        )
          throw new Error("invalid snapshot sequence");
        const paneBytes = (pendingProjection.paneBytes.get(frame.pane_id) ?? 0) + frame.data.length;
        pendingProjection.totalBytes += frame.data.length;
        if (
          paneBytes > ATTACHMENT_MAX_PANE_SNAPSHOT_BYTES ||
          pendingProjection.totalBytes > ATTACHMENT_MAX_PROJECTION_BYTES
        )
          throw new Error("projection exceeds limit");
        pendingProjection.paneBytes.set(frame.pane_id, paneBytes);
        pendingProjection.nextChunk.set(frame.pane_id, frame.chunk_index + 1);
        if (frame.final) pendingProjection.completed.add(frame.pane_id);
        return true;
      }
      if (frame.type === "workspace.output") {
        return frame.workspace_epoch === currentWorkspaceEpoch && currentPaneIds.has(frame.pane_id);
      }
      return true;
    };

    const send = (frame: object) => {
      if (socket.readyState !== WebSocket.OPEN) throw new Error("Attachment is not open");
      socket.send(JSON.stringify(frame));
    };
    socket.addEventListener("open", () => {
      if (disposed) socket.close(1000, "disposed");
      else send({ type: "auth.api_key", api_key: apiKey });
    });
    socket.addEventListener("message", (event) => {
      if (
        typeof event.data !== "string" ||
        UTF8_ENCODER.encode(event.data).length > ATTACHMENT_MAX_FRAME_BYTES
      ) {
        socket.close(1002, "invalid frame");
        return;
      }
      try {
        const frame = parseAttachmentFrame(JSON.parse(event.data) as unknown);
        if (validateSequence(frame)) onFrame(frame);
      } catch {
        socket.close(1002, "invalid frame");
      }
    });
    const close = () => {
      sockets.delete(socket);
      if (!closed) {
        closed = true;
        onClose();
      }
    };
    socket.addEventListener("close", close);
    socket.addEventListener("error", close);
    return {
      detach: () => send({ type: "workspace.detach" }),
      dispose: () => {
        sockets.delete(socket);
        socket.close(1000, "disposed");
      },
      returnToChooser: () => send({ type: "workspace.return_to_chooser" }),
      selectSession: (selectionEpoch, sessionId, sessionCreated) =>
        send({
          type: "session.select",
          selection_epoch: selectionEpoch,
          session_id: sessionId,
          session_created: sessionCreated,
        }),
    };
  }

  return {
    cancelEnrollment: (machineId) =>
      request(`/api/v1/machines/${encodeURIComponent(machineId)}/enrollment-token`, {
        method: "DELETE",
      }),
    createCredential: (input) =>
      request("/api/v1/ssh-credentials", { method: "POST", body: JSON.stringify(input) }),
    createMachine: (input) =>
      request("/api/v1/machines", { method: "POST", body: JSON.stringify(input) }),
    deployment: () => request("/api/v1/deployment"),
    disableMachine: (machineId) =>
      request(`/api/v1/machines/${encodeURIComponent(machineId)}/disable`, { method: "POST" }),
    dispose,
    enableMachine: (machineId) =>
      request(`/api/v1/machines/${encodeURIComponent(machineId)}/enable`, { method: "POST" }),
    issueEnrollment: (machineId) =>
      request(`/api/v1/machines/${encodeURIComponent(machineId)}/enrollment-token`, {
        method: "POST",
      }),
    reEnrollMachine: (machineId) =>
      request(`/api/v1/machines/${encodeURIComponent(machineId)}/re-enroll`, {
        method: "POST",
      }),
    openAttachment,
    listCredentials: () => request("/api/v1/ssh-credentials"),
    listMachines: () => request("/api/v1/machines"),
    renameCredential: (credentialId, name) =>
      request(`/api/v1/ssh-credentials/${encodeURIComponent(credentialId)}`, {
        method: "PATCH",
        body: JSON.stringify({ name }),
      }),
    resetDefaultCredential: (name) =>
      request("/api/v1/ssh-credentials/reset", {
        method: "POST",
        body: JSON.stringify({ name }),
      }),
    retireCredential: (credentialId) =>
      request(`/api/v1/ssh-credentials/${encodeURIComponent(credentialId)}/retire`, {
        method: "POST",
      }),
    setDefaultCredential: (credentialId) =>
      request(`/api/v1/ssh-credentials/${encodeURIComponent(credentialId)}/default`, {
        method: "POST",
      }),
  };
}
