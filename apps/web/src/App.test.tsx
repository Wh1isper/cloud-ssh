import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { App } from "./App";

describe("App", () => {
  it("states the target-owned tmux boundary", () => {
    const markup = renderToStaticMarkup(<App />);

    expect(markup).toContain("Your tmux sessions stay where they belong.");
    expect(markup).toContain("Foundation");
    expect(markup).toContain("contains no login");
  });
});
