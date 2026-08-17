import { afterEach, describe, expect, it, vi } from "vitest";

import { ApiError, AuthenticationError, createApiClient, parseAttachmentFrame } from "./client";

const workspaceEpoch = "123e4567-e89b-42d3-a456-426614174000";

function projection() {
  return {
    type: "workspace.projection",
    machine_connection_epoch: "7",
    workspace_epoch: workspaceEpoch,
    session_id: "$1",
    session_created: 1_700_000_000,
    windows: [{ window_id: "@1", name: "main", active: true }],
    window: {
      window_id: "@1",
      name: "main",
      width: 160,
      height: 48,
      layout: "abcd,160x48,0,0{79x48,0,0,1,80x48,80,0,2}",
    },
    panes: [
      {
        pane_id: "%1",
        active: true,
        width: 79,
        height: 48,
        left: 0,
        top: 0,
        title: "shell",
        current_command: "bash",
      },
      {
        pane_id: "%2",
        active: false,
        width: 80,
        height: 48,
        left: 80,
        top: 0,
        title: "worker",
        current_command: "sh",
      },
    ],
  };
}

class FakeWebSocket {
  static readonly OPEN = 1;
  static readonly instances: Array<FakeWebSocket> = [];

  readonly sent: Array<string> = [];
  readonly closed: Array<{ code?: number; reason?: string }> = [];
  readonly listeners = new Map<string, Array<(event: { data?: unknown }) => void>>();
  readonly bufferedAmount = 0;
  readonly readyState = FakeWebSocket.OPEN;

  constructor(readonly url: URL) {
    FakeWebSocket.instances.push(this);
  }

  addEventListener(type: string, listener: (event: { data?: unknown }) => void): void {
    const listeners = this.listeners.get(type) ?? [];
    listeners.push(listener);
    this.listeners.set(type, listeners);
  }

  send(value: string): void {
    this.sent.push(value);
  }

  close(code?: number, reason?: string): void {
    this.closed.push({ code, reason });
  }

  emit(type: string, event: { data?: unknown } = {}): void {
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }
}

function installClientEnvironment(): void {
  FakeWebSocket.instances.length = 0;
  vi.stubGlobal("window", {
    location: { origin: "http://owlmux.test", protocol: "http:" },
  });
  vi.stubGlobal("WebSocket", FakeWebSocket);
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("attachment frame parser", () => {
  it("accepts a closed target-authoritative two-pane projection", () => {
    const frame = parseAttachmentFrame(projection());

    expect(frame.type).toBe("workspace.projection");
    if (frame.type === "workspace.projection") {
      expect(frame.panes.map((pane) => pane.pane_id)).toEqual(["%1", "%2"]);
      expect(frame.window.width).toBe(160);
    }
  });

  it("rejects duplicate panes, impossible layout, and unknown fields", () => {
    const duplicate = projection();
    duplicate.panes[1].pane_id = "%1";
    expect(() => parseAttachmentFrame(duplicate)).toThrow();

    const impossible = projection();
    impossible.panes[1].left = 100;
    expect(() => parseAttachmentFrame(impossible)).toThrow();

    expect(() => parseAttachmentFrame({ ...projection(), target_command: "rm -rf /" })).toThrow();
  });

  it("rejects a target grid that exceeds the renderer budget", () => {
    const oversized = projection();
    oversized.window.width = 2_000;
    oversized.window.height = 2_000;
    oversized.panes = [
      {
        pane_id: "%1",
        active: true,
        width: 2_000,
        height: 2_000,
        left: 0,
        top: 0,
        title: "shell",
        current_command: "bash",
      },
    ];
    expect(() => parseAttachmentFrame(oversized)).toThrow();
  });

  it("accepts writer state and exact operation results", () => {
    expect(
      parseAttachmentFrame({
        type: "writer.state",
        machine_connection_epoch: "7",
        attachment_epoch: workspaceEpoch,
        role: "writer",
        writer_available: false,
      }),
    ).toMatchObject({ role: "writer" });
    expect(
      parseAttachmentFrame({
        type: "operation.result",
        request_id: "123e4567-e89b-42d3-a456-426614174001",
        operation: "pane.input",
        outcome: "ambiguous",
        code: "operation_ambiguous",
        message: "Input may have reached target tmux.",
      }),
    ).toMatchObject({ outcome: "ambiguous" });
  });

  it("rejects malformed and oversized terminal chunks without assuming UTF-8", () => {
    const valid = parseAttachmentFrame({
      type: "workspace.output",
      workspace_epoch: workspaceEpoch,
      pane_id: "%1",
      data_base64: "_w",
    });
    expect(valid.type === "workspace.output" && valid.data[0]).toBe(255);

    expect(() =>
      parseAttachmentFrame({
        type: "workspace.output",
        workspace_epoch: workspaceEpoch,
        pane_id: "%1",
        data_base64: "a".repeat(21_850),
      }),
    ).toThrow();
    expect(() =>
      parseAttachmentFrame({
        type: "workspace.output",
        workspace_epoch: workspaceEpoch,
        pane_id: "%1",
        data_base64: "not+base64",
      }),
    ).toThrow();
    expect(() =>
      parseAttachmentFrame({
        type: "workspace.output",
        workspace_epoch: workspaceEpoch,
        pane_id: "%1",
        data_base64: "_x",
      }),
    ).toThrow();
  });
});

describe("API client lifecycle", () => {
  it("disposes HTTP and WebSocket work on attachment authentication failure", async () => {
    installClientEnvironment();
    const requestSignals: Array<AbortSignal> = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((_input: RequestInfo | URL, init?: RequestInit) => {
        const signal = init?.signal;
        if (signal) requestSignals.push(signal);
        return new Promise<Response>((_resolve, reject) => {
          signal?.addEventListener("abort", () =>
            reject(new DOMException("aborted", "AbortError")),
          );
        });
      }),
    );
    const client = createApiClient("owlmux_sk_v1_YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE");
    const pendingRequest = client.deployment().catch((error: unknown) => error);
    const authenticationFailure = vi.fn();
    client.openAttachment(
      "machine-a",
      () => undefined,
      () => undefined,
      authenticationFailure,
    );
    const socket = FakeWebSocket.instances[0];
    expect(socket).toBeDefined();
    socket.emit("open");
    socket.emit("message", {
      data: JSON.stringify({
        type: "workspace.error",
        code: "unauthenticated",
        message: "Authentication failed.",
      }),
    });

    expect(requestSignals[0]?.aborted).toBe(true);
    expect(socket.closed).toContainEqual({ code: 1000, reason: "logout" });
    expect(authenticationFailure).toHaveBeenCalledOnce();
    expect(await pendingRequest).toBeInstanceOf(DOMException);
    await expect(client.listMachines()).rejects.toBeInstanceOf(AuthenticationError);
  });

  it("preserves typed API errors and unknown-outcome semantics", async () => {
    installClientEnvironment();
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response(
            JSON.stringify({
              code: "operation_ambiguous",
              message: "Operation outcome is unknown; refresh before deciding what to do.",
            }),
            { status: 503, headers: { "Content-Type": "application/json" } },
          ),
      ),
    );
    const client = createApiClient("owlmux_sk_v1_YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE");

    const error = await client.revokeRelay("machine-a").catch((reason: unknown) => reason);

    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({ code: "operation_ambiguous", outcomeUnknown: true, status: 503 });
  });

  it("classifies interrupted and untyped mutation responses as unknown outcomes", async () => {
    installClientEnvironment();
    const fetch = vi
      .fn()
      .mockRejectedValueOnce(new TypeError("connection closed"))
      .mockResolvedValueOnce(new Response(null, { status: 408 }));
    vi.stubGlobal("fetch", fetch);
    const client = createApiClient("owlmux_sk_v1_YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE");

    const interrupted = await client.disableMachine("machine-a").catch((reason: unknown) => reason);
    const untyped = await client.revokeRelay("machine-a").catch((reason: unknown) => reason);

    expect(interrupted).toMatchObject({
      code: "operation_ambiguous",
      outcomeUnknown: true,
      status: 0,
    });
    expect(untyped).toMatchObject({
      code: "operation_ambiguous",
      outcomeUnknown: true,
      status: 408,
    });
  });

  it("uses closed same-origin routes for Machine lifecycle controls", async () => {
    installClientEnvironment();
    const calls: Array<{ method: string; path: string; body: string | null }> = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = new URL(String(input));
        calls.push({
          method: init?.method ?? "GET",
          path: url.pathname,
          body: typeof init?.body === "string" ? init.body : null,
        });
        return new Response(null, { status: 204 });
      }),
    );
    const client = createApiClient("owlmux_sk_v1_YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE");

    await client.renameMachine("machine-a", "renamed");
    await client.rebindMachine("machine-a", "credential-b");
    await client.cancelEnrollment("machine-a");
    await client.revokeRelay("machine-a");

    expect(calls).toEqual([
      { method: "PATCH", path: "/api/v1/machines/machine-a", body: '{"alias":"renamed"}' },
      {
        method: "PATCH",
        path: "/api/v1/machines/machine-a/ssh-credential",
        body: '{"ssh_credential_id":"credential-b"}',
      },
      {
        method: "DELETE",
        path: "/api/v1/machines/machine-a/enrollment-token",
        body: null,
      },
      { method: "POST", path: "/api/v1/machines/machine-a/relay/revoke", body: null },
    ]);
  });

  it("releases operation and input budgets on epoch replacement without late-result underflow", () => {
    installClientEnvironment();
    const client = createApiClient("owlmux_sk_v1_YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE");
    const session = client.openAttachment(
      "machine-a",
      () => undefined,
      () => undefined,
      () => undefined,
    );
    const socket = FakeWebSocket.instances[0];
    expect(socket).toBeDefined();
    socket.emit("open");

    for (let index = 0; index < 32; index += 1) {
      session.refreshSessions("7", workspaceEpoch);
    }
    expect(() => session.refreshSessions("7", workspaceEpoch)).toThrow(
      "Too many target operations",
    );
    socket.emit("message", {
      data: JSON.stringify({ type: "workspace.phase", phase: "connecting" }),
    });

    const input = new Uint8Array(1024);
    const staleRequestIds = Array.from({ length: 8 }, () =>
      session.sendPaneInput("7", workspaceEpoch, "%1", input),
    );
    expect(() => session.sendPaneInput("7", workspaceEpoch, "%1", input)).toThrow(
      "Too much pane input",
    );
    socket.emit("message", {
      data: JSON.stringify({ type: "workspace.phase", phase: "connecting" }),
    });
    for (const requestId of staleRequestIds) {
      socket.emit("message", {
        data: JSON.stringify({
          type: "operation.result",
          request_id: requestId,
          operation: "pane.input",
          outcome: "failed",
          code: "stale_epoch",
          message: "The workspace changed.",
        }),
      });
    }
    for (let index = 0; index < 8; index += 1) {
      session.sendPaneInput("7", workspaceEpoch, "%1", input);
    }
    expect(() => session.sendPaneInput("7", workspaceEpoch, "%1", input)).toThrow(
      "Too much pane input",
    );

    socket.emit("message", {
      data: JSON.stringify({
        type: "session.list",
        machine_connection_epoch: "8",
        selection_epoch: workspaceEpoch,
        tmux_client_version: "tmux 3.2a",
        tmux_server_version: "tmux 3.2a",
        sessions: [],
      }),
    });
    expect(() => session.sendPaneInput("8", workspaceEpoch, "%1", input)).not.toThrow();
  });
});
