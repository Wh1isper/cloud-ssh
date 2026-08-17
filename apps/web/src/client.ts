import {
  ATTACHMENT_CLOSE_NORMAL,
  ATTACHMENT_CLOSE_PROTOCOL_ERROR,
  ATTACHMENT_MAX_FRAME_BYTES,
  ATTACHMENT_MAX_PANE_SNAPSHOT_BYTES,
  ATTACHMENT_MAX_PENDING_INPUT_BYTES,
  ATTACHMENT_MAX_PENDING_OPERATIONS,
  ATTACHMENT_MAX_PROJECTION_BYTES,
  ATTACHMENT_MAX_SOCKET_BUFFER_BYTES,
} from "./generated/contracts";
import {
  encodeBase64Url,
  parseAttachmentFrame,
  validateAttachmentInput,
  type AttachmentFrame,
} from "./attachment";
import type {
  AuditEventSummary,
  CreateCredentialInput,
  CreateMachineInput,
  CredentialSummary,
  DeploymentPresentation,
  EnrollmentTokenResponse,
  ErrorCode,
  ErrorResponse,
  MachineCreated,
  MachineSummary,
} from "./generated/contracts";

export class AuthenticationError extends Error {}

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: ErrorCode,
    message: string,
    readonly retryAfter?: number,
  ) {
    super(message);
  }

  get outcomeUnknown(): boolean {
    return this.code === "operation_ambiguous";
  }
}

const ERROR_CODES = new Set<ErrorCode>([
  "not_implemented",
  "unauthenticated",
  "invalid_name",
  "invalid_target_account",
  "invalid_tmux_path",
  "invalid_tmux_socket",
  "invalid_host_identity",
  "not_found",
  "credential_in_use",
  "credential_limit",
  "machine_limit",
  "invalid_lifecycle",
  "conflict",
  "temporarily_unavailable",
  "owner_unreachable",
  "operation_ambiguous",
  "internal_error",
]);
const UTF8_ENCODER = new TextEncoder();

function parseErrorResponse(value: unknown): ErrorResponse | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  const keys = Object.keys(record);
  if (
    keys.some((key) => !["code", "message", "retry_after"].includes(key)) ||
    typeof record.code !== "string" ||
    !ERROR_CODES.has(record.code as ErrorCode) ||
    typeof record.message !== "string" ||
    record.message.length === 0 ||
    record.message.length > 256 ||
    (record.retry_after !== undefined &&
      (!Number.isInteger(record.retry_after) ||
        (record.retry_after as number) < 1 ||
        (record.retry_after as number) > 30))
  ) {
    return null;
  }
  return {
    code: record.code as ErrorCode,
    message: record.message,
    ...(record.retry_after === undefined ? {} : { retry_after: record.retry_after as number }),
  };
}

export { parseAttachmentFrame } from "./attachment";
export type {
  AttachmentFrame,
  AttachmentOperation,
  AttachmentPane,
  AttachmentSessionSummary,
  AttachmentWindow,
  AttachmentWindowSummary,
} from "./attachment";

export interface AttachmentSession {
  claimWriter(
    machineConnectionEpoch: string,
    attachmentEpoch: string,
    columns: number,
    rows: number,
  ): string;
  createSession(machineConnectionEpoch: string, selectionEpoch: string, name: string): string;
  detach(): void;
  dispose(): void;
  refresh(machineConnectionEpoch: string, workspaceEpoch: string): string;
  refreshSessions(machineConnectionEpoch: string, selectionEpoch: string): string;
  resize(
    machineConnectionEpoch: string,
    workspaceEpoch: string,
    columns: number,
    rows: number,
  ): string;
  returnToChooser(machineConnectionEpoch: string, workspaceEpoch: string): void;
  selectPane(machineConnectionEpoch: string, workspaceEpoch: string, paneId: string): string;
  selectSession(
    machineConnectionEpoch: string,
    selectionEpoch: string,
    sessionId: string,
    sessionCreated: number,
  ): void;
  selectWindow(machineConnectionEpoch: string, workspaceEpoch: string, windowId: string): string;
  sendPaneInput(
    machineConnectionEpoch: string,
    workspaceEpoch: string,
    paneId: string,
    data: Uint8Array,
  ): string;
  takeOverWriter(
    machineConnectionEpoch: string,
    attachmentEpoch: string,
    columns: number,
    rows: number,
  ): string;
}

export interface ApiClient {
  cancelEnrollment(machineId: string): Promise<void>;
  auditEvents(): Promise<Array<AuditEventSummary>>;
  createCredential(input: CreateCredentialInput): Promise<CredentialSummary>;
  createMachine(input: CreateMachineInput): Promise<MachineCreated>;
  deployment(): Promise<DeploymentPresentation>;
  disableMachine(machineId: string): Promise<void>;
  dispose(): void;
  issueEnrollment(machineId: string): Promise<EnrollmentTokenResponse>;
  rebindMachine(machineId: string, credentialId: string): Promise<void>;
  reEnrollMachine(machineId: string): Promise<void>;
  renameMachine(machineId: string, alias: string): Promise<void>;
  revokeRelay(machineId: string): Promise<void>;
  openAttachment(
    machineId: string,
    onFrame: (frame: AttachmentFrame) => void,
    onClose: () => void,
    onAuthenticationFailure: () => void,
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
    const method = (init.method ?? "GET").toUpperCase();
    const mutating = method !== "GET" && method !== "HEAD";
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
        const value = (await response.json().catch(() => null)) as unknown;
        const body = parseErrorResponse(value);
        if (body !== null) {
          throw new ApiError(response.status, body.code, body.message, body.retry_after);
        }
        if (mutating) {
          throw new ApiError(
            response.status,
            "operation_ambiguous",
            "The mutation response was incomplete; its durable outcome is unknown.",
          );
        }
        throw new Error(`Request failed (${response.status})`);
      }
      if (response.status === 204) {
        return undefined as T;
      }
      return (await response.json()) as T;
    } catch (error) {
      if (error instanceof AuthenticationError || error instanceof ApiError || !mutating) {
        throw error;
      }
      throw new ApiError(
        0,
        "operation_ambiguous",
        "The mutation response was interrupted; its durable outcome is unknown.",
      );
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
    for (const socket of sockets) socket.close(ATTACHMENT_CLOSE_NORMAL, "logout");
    sockets.clear();
  }

  function openAttachment(
    machineId: string,
    onFrame: (frame: AttachmentFrame) => void,
    onClose: () => void,
    onAuthenticationFailure: () => void,
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
    let currentConnectionEpoch: string | null = null;
    let currentAttachmentEpoch: string | null = null;
    let currentWorkspaceEpoch: string | null = null;
    let currentPaneIds = new Set<string>();
    const pendingOperations = new Map<string, number>();
    let pendingInputBytes = 0;

    const clearPendingOperations = () => {
      pendingOperations.clear();
      pendingInputBytes = 0;
    };

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
          clearPendingOperations();
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
        currentConnectionEpoch = frame.machine_connection_epoch;
        currentAttachmentEpoch = frame.selection_epoch;
        currentWorkspaceEpoch = null;
        currentPaneIds = new Set();
        clearPendingOperations();
        return true;
      }
      if (frame.type === "writer.state") {
        return (
          frame.machine_connection_epoch === currentConnectionEpoch &&
          frame.attachment_epoch === currentAttachmentEpoch
        );
      }
      if (frame.type === "workspace.projection") {
        if (frame.machine_connection_epoch !== currentConnectionEpoch)
          throw new Error("projection changed Machine connection epoch");
        const paneIds = new Set(frame.panes.map((pane) => pane.pane_id));
        currentAttachmentEpoch = frame.workspace_epoch;
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
      const encoded = JSON.stringify(frame);
      const encodedBytes = UTF8_ENCODER.encode(encoded).length;
      if (encodedBytes > ATTACHMENT_MAX_FRAME_BYTES)
        throw new Error("Attachment frame exceeds its bound");
      if (socket.bufferedAmount + encodedBytes > ATTACHMENT_MAX_SOCKET_BUFFER_BYTES)
        throw new Error("Attachment send buffer is full; input was not queued");
      socket.send(encoded);
    };
    const request = (type: string, fields: object, inputBytes = 0): string => {
      if (pendingOperations.size >= ATTACHMENT_MAX_PENDING_OPERATIONS)
        throw new Error("Too many target operations are pending; input was not queued");
      if (pendingInputBytes + inputBytes > ATTACHMENT_MAX_PENDING_INPUT_BYTES)
        throw new Error("Too much pane input is pending; input was not queued");
      const requestId = crypto.randomUUID();
      send({ type, request_id: requestId, ...fields });
      pendingOperations.set(requestId, inputBytes);
      pendingInputBytes += inputBytes;
      return requestId;
    };
    socket.addEventListener("open", () => {
      if (disposed) socket.close(ATTACHMENT_CLOSE_NORMAL, "disposed");
      else send({ type: "auth.api_key", api_key: apiKey });
    });
    socket.addEventListener("message", (event) => {
      if (
        typeof event.data !== "string" ||
        UTF8_ENCODER.encode(event.data).length > ATTACHMENT_MAX_FRAME_BYTES
      ) {
        socket.close(ATTACHMENT_CLOSE_PROTOCOL_ERROR, "invalid frame");
        return;
      }
      try {
        const frame = parseAttachmentFrame(JSON.parse(event.data) as unknown);
        if (frame.type === "workspace.error" && frame.code === "unauthenticated") {
          dispose();
          onAuthenticationFailure();
          return;
        }
        if (frame.type === "operation.result") {
          const inputBytes = pendingOperations.get(frame.request_id) ?? 0;
          pendingOperations.delete(frame.request_id);
          pendingInputBytes -= inputBytes;
        }
        if (validateSequence(frame)) onFrame(frame);
      } catch {
        socket.close(ATTACHMENT_CLOSE_PROTOCOL_ERROR, "invalid frame");
      }
    });
    const close = () => {
      clearPendingOperations();
      sockets.delete(socket);
      if (!closed) {
        closed = true;
        onClose();
      }
    };
    socket.addEventListener("close", close);
    socket.addEventListener("error", close);
    return {
      claimWriter: (machineConnectionEpoch, attachmentEpoch, columns, rows) =>
        request("writer.claim", {
          machine_connection_epoch: machineConnectionEpoch,
          attachment_epoch: attachmentEpoch,
          columns,
          rows,
        }),
      createSession: (machineConnectionEpoch, selectionEpoch, name) =>
        request("session.create", {
          machine_connection_epoch: machineConnectionEpoch,
          selection_epoch: selectionEpoch,
          name,
        }),
      detach: () => send({ type: "workspace.detach" }),
      dispose: () => {
        sockets.delete(socket);
        socket.close(ATTACHMENT_CLOSE_NORMAL, "disposed");
      },
      refresh: (machineConnectionEpoch, workspaceEpoch) =>
        request("workspace.refresh", {
          machine_connection_epoch: machineConnectionEpoch,
          workspace_epoch: workspaceEpoch,
        }),
      refreshSessions: (machineConnectionEpoch, selectionEpoch) =>
        request("session.refresh", {
          machine_connection_epoch: machineConnectionEpoch,
          selection_epoch: selectionEpoch,
        }),
      resize: (machineConnectionEpoch, workspaceEpoch, columns, rows) =>
        request("client.resize", {
          machine_connection_epoch: machineConnectionEpoch,
          workspace_epoch: workspaceEpoch,
          columns,
          rows,
        }),
      returnToChooser: (machineConnectionEpoch, workspaceEpoch) =>
        send({
          type: "workspace.return_to_chooser",
          machine_connection_epoch: machineConnectionEpoch,
          workspace_epoch: workspaceEpoch,
        }),
      selectPane: (machineConnectionEpoch, workspaceEpoch, paneId) =>
        request("pane.select", {
          machine_connection_epoch: machineConnectionEpoch,
          workspace_epoch: workspaceEpoch,
          pane_id: paneId,
        }),
      selectSession: (machineConnectionEpoch, selectionEpoch, sessionId, sessionCreated) =>
        send({
          type: "session.select",
          machine_connection_epoch: machineConnectionEpoch,
          selection_epoch: selectionEpoch,
          session_id: sessionId,
          session_created: sessionCreated,
        }),
      selectWindow: (machineConnectionEpoch, workspaceEpoch, windowId) =>
        request("window.select", {
          machine_connection_epoch: machineConnectionEpoch,
          workspace_epoch: workspaceEpoch,
          window_id: windowId,
        }),
      sendPaneInput: (machineConnectionEpoch, workspaceEpoch, paneId, data) => {
        validateAttachmentInput(data);
        return request(
          "pane.input",
          {
            machine_connection_epoch: machineConnectionEpoch,
            workspace_epoch: workspaceEpoch,
            pane_id: paneId,
            data_base64: encodeBase64Url(data),
          },
          data.length,
        );
      },
      takeOverWriter: (machineConnectionEpoch, attachmentEpoch, columns, rows) =>
        request("writer.takeover", {
          machine_connection_epoch: machineConnectionEpoch,
          attachment_epoch: attachmentEpoch,
          columns,
          rows,
        }),
    };
  }

  return {
    auditEvents: () => request("/api/v1/audit-events"),
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
    issueEnrollment: (machineId) =>
      request(`/api/v1/machines/${encodeURIComponent(machineId)}/enrollment-token`, {
        method: "POST",
      }),
    rebindMachine: (machineId, credentialId) =>
      request(`/api/v1/machines/${encodeURIComponent(machineId)}/ssh-credential`, {
        method: "PATCH",
        body: JSON.stringify({ ssh_credential_id: credentialId }),
      }),
    reEnrollMachine: (machineId) =>
      request(`/api/v1/machines/${encodeURIComponent(machineId)}/re-enroll`, {
        method: "POST",
      }),
    renameMachine: (machineId, alias) =>
      request(`/api/v1/machines/${encodeURIComponent(machineId)}`, {
        method: "PATCH",
        body: JSON.stringify({ alias }),
      }),
    revokeRelay: (machineId) =>
      request(`/api/v1/machines/${encodeURIComponent(machineId)}/relay/revoke`, {
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
