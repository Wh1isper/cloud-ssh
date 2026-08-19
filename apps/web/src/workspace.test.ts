import { describe, expect, it } from "vitest";

import {
  attachmentCloseMessage,
  attachmentErrorMessage,
  isCurrentAttachmentAttempt,
} from "./workspace";

describe("workspace attachment lifecycle", () => {
  it("ignores callbacks from a disposed or replaced connection attempt", () => {
    const oldAttempt = {};
    const replacement = {};

    expect(isCurrentAttachmentAttempt(false, oldAttempt, oldAttempt)).toBe(true);
    expect(isCurrentAttachmentAttempt(false, oldAttempt, replacement)).toBe(false);
    expect(isCurrentAttachmentAttempt(true, oldAttempt, oldAttempt)).toBe(false);
  });

  it("retains a fatal diagnostic when the socket closes afterward", () => {
    expect(attachmentCloseMessage("Target tmux is not compatible.")).toBe(
      "Target tmux is not compatible.",
    );
    expect(attachmentCloseMessage(null)).toBe(
      "The OwlMux connection ended. Target tmux and its processes continue on the Host.",
    );
  });

  it("adds the required operator recovery action for an unreachable owner", () => {
    expect(attachmentErrorMessage("owner_unreachable", "Owner unavailable.")).toBe(
      "The valid Host owner is unreachable. Fence or stop that Server node, wait for lease expiry, then reconnect.",
    );
  });

  it("preserves reviewed Server diagnostics for other attachment failures", () => {
    expect(attachmentErrorMessage("tmux_incompatible", "Target tmux is not compatible.")).toBe(
      "Target tmux is not compatible.",
    );
  });
});
