import { describe, expect, it } from "vitest";

import { parseAttachmentFrame } from "./client";

const workspaceEpoch = "123e4567-e89b-42d3-a456-426614174000";

function projection() {
  return {
    type: "workspace.projection",
    workspace_epoch: workspaceEpoch,
    session_id: "$1",
    session_created: 1_700_000_000,
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
