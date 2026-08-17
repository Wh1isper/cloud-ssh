/* global document, HTMLElement */

import { chromium } from "@playwright/test";

const server = process.env.OWLMUX_E2E_HTTP_SERVER;
const apiKey = process.env.OWLMUX_E2E_API_KEY;
if (!server || !apiKey) throw new Error("missing Browser smoke configuration");

const browser = await chromium.launch({ headless: true });
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
const browserErrors = [];
page.on("console", (message) => {
  if (message.type() === "error") browserErrors.push(message.text());
});
page.on("pageerror", (error) => browserErrors.push(error.message));

try {
  const response = await page.goto(server, { waitUntil: "networkidle" });
  if (response === null || !response.ok()) throw new Error("product page did not load");
  const csp = response.headers()["content-security-policy"] ?? "";
  if (!csp.includes("style-src 'self' 'unsafe-inline'")) {
    throw new Error("product CSP does not permit reviewed React/xterm styles");
  }

  await page.getByLabel("Deployment API key").fill(apiKey);
  await page.getByRole("button", { name: "Open deployment" }).click();
  await page.getByText("e2e-target", { exact: true }).waitFor();
  await page.getByRole("button", { name: "Open read-only workspace" }).click();
  await page.getByRole("heading", { name: "Choose a current target session" }).waitFor();
  await page.getByRole("button", { name: "Open read-only", exact: true }).click();
  const layout = page.getByLabel("Target-authoritative tmux pane layout");
  await layout.waitFor();
  await page.getByText("2 visible panes", { exact: false }).waitFor();
  await page.locator(".tmux-pane").first().waitFor();

  const geometry = await page.evaluate(() => {
    const layoutElement = document.querySelector(
      '[aria-label="Target-authoritative tmux pane layout"]',
    );
    const panes = [...document.querySelectorAll(".tmux-pane")];
    if (!(layoutElement instanceof HTMLElement)) return null;
    const layoutRect = layoutElement.getBoundingClientRect();
    return {
      layout: {
        width: layoutRect.width,
        height: layoutRect.height,
      },
      panes: panes.map((pane) => {
        const rect = pane.getBoundingClientRect();
        return {
          width: rect.width,
          height: rect.height,
          left: rect.left - layoutRect.left,
          top: rect.top - layoutRect.top,
          terminalRows: pane.querySelectorAll(".xterm-rows > div").length,
        };
      }),
      terminalText: panes.map((pane) => pane.querySelector(".xterm-rows")?.textContent ?? ""),
    };
  });
  if (geometry === null || geometry.layout.width <= 0 || geometry.layout.height <= 0) {
    throw new Error("target layout has no rendered geometry");
  }
  if (geometry.panes.length !== 2) throw new Error("Browser did not render two tmux panes");
  for (const pane of geometry.panes) {
    if (
      pane.width <= 0 ||
      pane.height <= 0 ||
      pane.left < 0 ||
      pane.top < 0 ||
      pane.left + pane.width > geometry.layout.width + 1 ||
      pane.top + pane.height > geometry.layout.height + 1 ||
      pane.terminalRows <= 0
    ) {
      throw new Error("pane geometry escaped the target-authoritative layout");
    }
  }
  if (!geometry.terminalText.some((text) => text.includes("primary-ready"))) {
    throw new Error("xterm did not render the target snapshot");
  }
  if (browserErrors.length !== 0) {
    throw new Error(`Browser reported errors: ${browserErrors.join(" | ")}`);
  }

  await page.reload({ waitUntil: "networkidle" });
  await page.getByLabel("Deployment API key").waitFor();
  if (await page.getByText("e2e-target", { exact: true }).isVisible()) {
    throw new Error("reload retained Browser authentication state");
  }
  console.log(
    "Chromium CSP, two-pane xterm geometry, snapshot, and reload-memory behavior verified",
  );
} finally {
  await browser.close();
}
