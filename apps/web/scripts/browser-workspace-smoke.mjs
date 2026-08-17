/* global document, HTMLElement, requestAnimationFrame */

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

  const machineResponse = await fetch(`${server}/api/v1/machines`, {
    headers: { Authorization: `Bearer ${apiKey}` },
  });
  if (!machineResponse.ok) throw new Error("could not inspect Browser smoke Machine");
  const smokeMachine = (await machineResponse.json()).find(
    (machine) => machine.alias === "e2e-target",
  );
  if (!smokeMachine) throw new Error("Browser smoke Machine is missing");

  let releaseInitialRefresh;
  let markInitialRefreshCaptured;
  let markInitialRefreshFulfilled;
  let initialRefreshCaptured = false;
  const initialRefreshReady = new Promise((resolve) => {
    markInitialRefreshCaptured = resolve;
  });
  const initialRefreshFulfilled = new Promise((resolve) => {
    markInitialRefreshFulfilled = resolve;
  });
  let releaseFailedMutation;
  let markMutationStarted;
  const mutationStarted = new Promise((resolve) => {
    markMutationStarted = resolve;
  });
  await page.route("**/api/v1/machines", async (route) => {
    const method = route.request().method();
    if (method === "GET" && !initialRefreshCaptured) {
      initialRefreshCaptured = true;
      const response = await route.fetch();
      markInitialRefreshCaptured();
      await new Promise((resolve) => {
        releaseInitialRefresh = resolve;
      });
      await route.fulfill({
        response,
        headers: {
          ...response.headers(),
          "x-owlmux-e2e-stale-refresh": "1",
        },
      });
      markInitialRefreshFulfilled();
    } else if (method === "POST") {
      markMutationStarted();
      await new Promise((resolve) => {
        releaseFailedMutation = resolve;
      });
      await route.abort("connectionfailed");
    } else {
      await route.continue();
    }
  });

  await page.getByLabel("Deployment API key").fill(apiKey);
  await page.getByRole("button", { name: "Open deployment" }).click();
  await initialRefreshReady;

  const refreshedAlias = "e2e-target-refreshed";
  const renameResponse = await fetch(`${server}/api/v1/machines/${smokeMachine.machine_id}`, {
    method: "PATCH",
    headers: {
      Authorization: `Bearer ${apiKey}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ alias: refreshedAlias }),
  });
  if (!renameResponse.ok) throw new Error("could not prepare stale-refresh Browser case");

  await page.getByLabel("Alias").fill("discarded-unknown-machine");
  await page.getByLabel("Target account").fill("owlmux");
  await page
    .getByLabel("Expected SSH host public key")
    .fill("ssh-ed25519 invalid-for-aborted-request");
  await page.getByRole("button", { name: "Create pending Machine" }).click();
  await mutationStarted;
  if (!(await page.getByRole("button", { name: "Create pending Machine" }).isDisabled())) {
    throw new Error("in-flight mutation did not serialize mutation controls");
  }
  releaseFailedMutation();
  await page.getByRole("button", { name: "Refresh durable state" }).waitFor();
  const expectedNetworkError = browserErrors.findIndex((message) =>
    message.includes("ERR_CONNECTION_FAILED"),
  );
  if (expectedNetworkError < 0) throw new Error("mutation transport failure was not observed");
  browserErrors.splice(expectedNetworkError, 1);
  if (!(await page.getByRole("button", { name: "Create pending Machine" }).isDisabled())) {
    throw new Error("unknown mutation outcome did not disable mutation controls");
  }

  await page.getByRole("button", { name: "Refresh durable state" }).click();
  await page.getByRole("button", { name: "Refresh durable state" }).waitFor({ state: "hidden" });
  await page.getByText(refreshedAlias, { exact: true }).waitFor();
  const staleResponseReceived = page.waitForResponse(
    (response) => response.headers()["x-owlmux-e2e-stale-refresh"] === "1",
  );
  releaseInitialRefresh();
  const staleResponse = await staleResponseReceived;
  await staleResponse.finished();
  await initialRefreshFulfilled;
  await page.evaluate(
    () =>
      new Promise((resolve) => {
        requestAnimationFrame(() => requestAnimationFrame(resolve));
      }),
  );
  if (!(await page.getByText(refreshedAlias, { exact: true }).isVisible())) {
    throw new Error("stale refresh replaced explicit durable state");
  }
  await page.unroute("**/api/v1/machines");

  page.once("dialog", async (dialog) => {
    await dialog.accept("e2e-target");
  });
  await page
    .getByRole("listitem")
    .filter({ hasText: refreshedAlias })
    .getByRole("button", { name: "Rename" })
    .click();
  await page.getByText("e2e-target", { exact: true }).waitFor();
  await page.getByText("<img src=x onerror=globalThis.owlmuxXss=true>", { exact: true }).waitFor();
  const unsafePresentation = await page.evaluate(() => ({
    injectedImage: document.querySelector('img[src="x"]') !== null,
    executed: globalThis.owlmuxXss === true,
  }));
  if (unsafePresentation.injectedImage || unsafePresentation.executed) {
    throw new Error("untrusted Machine alias escaped React text rendering");
  }
  await page.getByRole("button", { name: "SSH credentials" }).click();
  await page.getByRole("heading", { name: "SSH credentials" }).waitFor();
  if (!page.url().endsWith("/ssh-credentials")) throw new Error("credential navigation lost route");
  await page.getByRole("button", { name: "Machines" }).click();
  await page.getByText("e2e-target", { exact: true }).waitFor();
  if (!page.url().endsWith("/machines")) throw new Error("Machine navigation lost route");
  if (await page.getByRole("button", { name: "Open workspace" }).isDisabled()) {
    throw new Error("durable refresh did not restore mutation controls");
  }
  await page.getByRole("button", { name: "Open workspace" }).click();
  await page.getByRole("heading", { name: "Choose a current target session" }).waitFor();
  await page.getByRole("button", { name: "Claim writer" }).click();
  await page.getByText("Target-owned tmux · writer", { exact: true }).waitFor();
  await page.getByRole("button", { name: "Open", exact: true }).click();
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

  const firstMarker = "OWLMUX_BROWSER_WRITER_OK";
  await page.locator('[aria-label^="Writable terminal"]').click();
  await page.keyboard.insertText(`printf '${firstMarker}\\n'`);
  await page.keyboard.press("Enter");
  await page.waitForFunction(
    (marker) =>
      [...document.querySelectorAll(".xterm-rows")].some((rows) =>
        rows.textContent?.includes(String(marker)),
      ),
    firstMarker,
  );

  const takeoverPage = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
  takeoverPage.on("console", (message) => {
    if (message.type() === "error") browserErrors.push(message.text());
  });
  takeoverPage.on("pageerror", (error) => browserErrors.push(error.message));
  await takeoverPage.goto(server, { waitUntil: "networkidle" });
  await takeoverPage.getByLabel("Deployment API key").fill(apiKey);
  await takeoverPage.getByRole("button", { name: "Open deployment" }).click();
  await takeoverPage.getByText("e2e-target", { exact: true }).waitFor();
  await takeoverPage.getByRole("button", { name: "Open workspace" }).click();
  await takeoverPage.getByRole("heading", { name: "Choose a current target session" }).waitFor();
  await takeoverPage.getByRole("button", { name: "Take over writer" }).click();
  await takeoverPage.getByText("Target-owned tmux · writer", { exact: true }).waitFor();
  await page.getByText("Target-owned tmux · observer", { exact: true }).waitFor();
  await page.waitForFunction(
    (marker) =>
      [...document.querySelectorAll(".xterm-rows")].some((rows) =>
        rows.textContent?.includes(String(marker)),
      ),
    firstMarker,
  );

  await takeoverPage.getByRole("button", { name: "Open", exact: true }).click();
  await takeoverPage.locator('[aria-label^="Writable terminal"]').waitFor();
  const takeoverMarker = "OWLMUX_BROWSER_TAKEOVER_OK";
  await takeoverPage.locator('[aria-label^="Writable terminal"]').click();
  await takeoverPage.keyboard.insertText(`printf '${takeoverMarker}\\n'`);
  await takeoverPage.keyboard.press("Enter");
  await takeoverPage.waitForFunction(
    (marker) =>
      [...document.querySelectorAll(".xterm-rows")].some((rows) =>
        rows.textContent?.includes(String(marker)),
      ),
    takeoverMarker,
  );
  await takeoverPage.close();

  if (browserErrors.length !== 0) {
    throw new Error(`Browser reported errors: ${browserErrors.join(" | ")}`);
  }

  await page.getByRole("button", { name: "Detach" }).click();
  await page.getByRole("button", { name: "Log out and clear key" }).click();
  await page.getByLabel("Deployment API key").waitFor();
  if (!page.url().endsWith("/login")) throw new Error("logout did not return to /login");
  await page.getByLabel("Deployment API key").fill(apiKey);
  await page.getByRole("button", { name: "Open deployment" }).click();
  await page.getByText("e2e-target", { exact: true }).waitFor();
  await page.reload({ waitUntil: "networkidle" });
  await page.getByLabel("Deployment API key").waitFor();
  if (await page.getByText("e2e-target", { exact: true }).isVisible()) {
    throw new Error("reload retained Browser authentication state");
  }
  console.log(
    "Chromium CSP, safe text, navigation/logout, mutation ambiguity reconciliation, xterm geometry, writer input, takeover without renderer rollback, and reload-memory behavior verified",
  );
} finally {
  await browser.close();
}
