import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { App } from "./App";

describe("App", () => {
  it("starts at the page-memory-only Deployment login boundary", () => {
    const markup = renderToStaticMarkup(<App />);

    expect(markup).toContain("Your target work stays where it belongs.");
    expect(markup).toContain("Deployment API key");
    expect(markup).toContain('type="password"');
    expect(markup).toContain("stays only in this page&#x27;s memory");
    expect(markup).toContain("Open your saved Hosts");
    expect(markup).not.toContain("localStorage");
  });
});
