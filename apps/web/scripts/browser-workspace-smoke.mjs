/* global document, HTMLElement, HTMLInputElement, requestAnimationFrame */

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

async function waitForStableGrid(targetPage) {
  await targetPage.waitForFunction(() => {
    const text =
      document.querySelector(".workspace-status-bar span:nth-child(2)")?.textContent ?? "";
    const grid = text.match(/\d+×\d+/)?.[0] ?? "";
    if (!grid) return false;
    const state = globalThis.__owlmuxGridStability;
    if (state?.grid !== grid) {
      globalThis.__owlmuxGridStability = { grid, since: performance.now() };
      return false;
    }
    return performance.now() - state.since >= 400;
  });
  return targetPage.locator(".workspace-status-bar span").nth(1).textContent();
}

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
  if (!machineResponse.ok) throw new Error("could not inspect Browser smoke Host");
  const smokeMachine = (await machineResponse.json()).find(
    (machine) => machine.alias === "e2e-target",
  );
  if (!smokeMachine) throw new Error("Browser smoke Host is missing");

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
  await page.getByRole("button", { name: "Open OwlMux" }).click();
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

  await page.getByRole("button", { name: "Hosts", exact: true }).click();
  await page.getByRole("button", { name: "Add Host" }).click();
  await page.getByLabel("Host name").fill("discarded-unknown-host");
  await page.getByLabel("Target account").fill("owlmux");
  await page
    .getByLabel("Expected SSH host public key")
    .fill("ssh-ed25519 invalid-for-aborted-request");
  const createHostButton = page.getByRole("button", { name: "Create Host and issue token" });
  await createHostButton.click();
  await mutationStarted;
  if (!(await createHostButton.isDisabled())) {
    throw new Error("in-flight mutation did not serialize mutation controls");
  }
  releaseFailedMutation();
  await page.getByRole("button", { name: "Refresh durable state" }).waitFor();
  const expectedNetworkError = browserErrors.findIndex((message) =>
    message.includes("ERR_CONNECTION_FAILED"),
  );
  if (expectedNetworkError < 0) throw new Error("mutation transport failure was not observed");
  browserErrors.splice(expectedNetworkError, 1);
  if (!(await createHostButton.isDisabled())) {
    throw new Error("unknown mutation outcome did not disable mutation controls");
  }

  await page.getByRole("button", { name: "Refresh durable state" }).click();
  await page.getByRole("button", { name: "Refresh durable state" }).waitFor({ state: "hidden" });
  await page.getByRole("button", { name: "Hosts", exact: true }).click();
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

  await page
    .getByRole("listitem")
    .filter({ hasText: refreshedAlias })
    .getByRole("button", { name: "Manage" })
    .click();
  await page.getByRole("heading", { name: refreshedAlias, exact: true }).waitFor();
  await page.getByRole("button", { name: "Rename" }).click();
  const renameDialog = page.getByRole("dialog", { name: `Rename ${refreshedAlias}` });
  await renameDialog.getByLabel("Host name").fill("e2e-target");
  await renameDialog.getByRole("button", { name: "Rename Host" }).click();
  await page.getByRole("heading", { name: "e2e-target", exact: true }).waitFor();

  await page.getByRole("button", { name: "Hosts", exact: true }).click();
  await page.getByText("<img src=x onerror=globalThis.owlmuxXss=true>", { exact: true }).waitFor();
  const unsafePresentation = await page.evaluate(() => ({
    injectedImage: document.querySelector('img[src="x"]') !== null,
    executed: globalThis.owlmuxXss === true,
  }));
  if (unsafePresentation.injectedImage || unsafePresentation.executed) {
    throw new Error("untrusted Host name escaped React text rendering");
  }

  await page.getByRole("button", { name: "Credentials" }).click();
  await page.getByRole("heading", { name: "SSH credentials" }).waitFor();
  if (!page.url().endsWith("/ssh-credentials")) throw new Error("credential navigation lost route");
  await page.getByRole("button", { name: "Hosts", exact: true }).click();
  await page.getByText("e2e-target", { exact: true }).waitFor();
  if (!page.url().endsWith("/hosts")) throw new Error("Host navigation lost route");

  await page.getByRole("button", { name: "Workspaces" }).click();
  await page.getByRole("heading", { name: "Continue on a saved Host" }).waitFor();
  await page.keyboard.press("Control+k");
  const focusedPlaceholder = await page.evaluate(() =>
    document.activeElement instanceof HTMLInputElement ? document.activeElement.placeholder : "",
  );
  if (focusedPlaceholder !== "Search Hosts")
    throw new Error("Workspace search shortcut did not focus");

  const workspaceCard = page.locator(".host-card").filter({ hasText: "e2e-target" });
  if (await workspaceCard.getByRole("button", { name: "Open" }).isDisabled()) {
    throw new Error("durable refresh did not restore Host controls");
  }
  await workspaceCard.getByRole("button", { name: "Open" }).click();
  await page.getByRole("heading", { name: "Choose a tmux session" }).waitFor();
  await page.getByRole("button", { name: "Take control" }).click();
  await page.locator(".control-state").getByText("You have control", { exact: true }).waitFor();
  await page
    .locator(".session-row")
    .filter({ hasText: "alpha" })
    .getByRole("button", { name: "Open", exact: true })
    .click();

  const layout = page.getByLabel("Target-authoritative tmux pane layout");
  await layout.waitFor();
  await page.getByText(/2 panes$/).waitFor();
  await page.locator(".tmux-pane").first().waitFor();

  const initialGridText = await waitForStableGrid(page);
  const initialGrid = initialGridText?.match(/\d+×\d+/)?.[0];
  if (!initialGrid) throw new Error("initial target-authoritative terminal grid was not shown");
  await page.setViewportSize({ width: 980, height: 720 });
  await page.waitForFunction((before) => {
    const text =
      document.querySelector(".workspace-status-bar span:nth-child(2)")?.textContent ?? "";
    const current = text.match(/\d+×\d+/)?.[0];
    return current !== undefined && current !== before;
  }, initialGrid);
  const resizedGridText = await waitForStableGrid(page);
  const resizedGrid = resizedGridText?.match(/\d+×\d+/)?.[0];
  if (!resizedGrid || resizedGrid === initialGrid) {
    throw new Error("viewport change did not produce a fresh target-authoritative terminal grid");
  }

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

  await page
    .locator(".primary-navigation")
    .getByRole("button", { name: "Hosts", exact: true })
    .click();
  await page
    .getByRole("listitem")
    .filter({ hasText: "e2e-target" })
    .getByRole("button", { name: "Open" })
    .click();
  await page.getByRole("heading", { name: "Choose a tmux session" }).waitFor();
  if ((await page.locator(".workspace-tabs > .workspace-tab").count()) !== 3) {
    throw new Error(
      "opening the same Host did not create an independent page-memory workspace tab",
    );
  }
  await page.getByRole("button", { name: "Take over" }).click();
  await page
    .locator(".workspace-slot.is-active .control-state")
    .getByText("You have control", { exact: true })
    .waitFor();
  const tabRoles = await page.locator(".workspace-slot .control-state").allTextContents();
  if (!tabRoles.includes("View only · controlled elsewhere")) {
    throw new Error("same-page takeover did not demote the prior Host tab to observer");
  }
  await page
    .locator(".session-row")
    .filter({ hasText: "alpha" })
    .getByRole("button", { name: "Open", exact: true })
    .click();
  const tabMarker = "OWLMUX_BROWSER_TAB_TAKEOVER_OK";
  await page.locator('.workspace-slot.is-active [aria-label^="Writable terminal"]').click();
  await page.keyboard.insertText(`printf '${tabMarker}\\n'`);
  await page.keyboard.press("Enter");
  await page.waitForFunction(
    (marker) =>
      [...document.querySelectorAll(".workspace-slot.is-active .xterm-rows")].some((rows) =>
        rows.textContent?.includes(String(marker)),
      ),
    tabMarker,
  );

  await page
    .locator(".workspace-tabs > .workspace-tab")
    .nth(1)
    .locator(".workspace-tab-label")
    .click();
  await page
    .locator(".workspace-slot.is-active .control-state")
    .getByText("View only · controlled elsewhere", { exact: true })
    .waitFor();
  const observerGridText = await waitForStableGrid(page);
  const observerGrid = observerGridText?.match(/\d+×\d+/)?.[0];
  if (!observerGrid) throw new Error("visible observer did not expose target geometry");
  await page.setViewportSize({ width: 760, height: 620 });
  await page.waitForTimeout(600);
  const unchangedObserverGrid = (
    await page.locator(".workspace-slot.is-active .workspace-status-bar span").nth(1).textContent()
  )?.match(/\d+×\d+/)?.[0];
  if (unchangedObserverGrid !== observerGrid) {
    throw new Error("visible observer or hidden writer changed target tmux geometry");
  }

  await page
    .locator(".workspace-tabs > .workspace-tab")
    .nth(2)
    .locator(".workspace-tab-label")
    .click();
  await page.waitForFunction(
    () =>
      document.querySelector(".workspace-slot.is-active .control-state")?.textContent ===
      "You have control",
  );
  try {
    await page.waitForFunction(
      (before) => {
        const text = document.querySelector(
          ".workspace-slot.is-active .workspace-status-bar span:nth-child(2)",
        )?.textContent;
        const current = text?.match(/\d+×\d+/)?.[0];
        return current !== undefined && current !== before;
      },
      observerGrid,
      { timeout: 8_000 },
    );
  } catch {
    const diagnostic = await page.evaluate(() => {
      const layout = document
        .querySelector(".workspace-slot.is-active .pane-layout")
        ?.getBoundingClientRect();
      return {
        alert: document.querySelector(".workspace-slot.is-active [role='alert']")?.textContent,
        grid: document.querySelector(
          ".workspace-slot.is-active .workspace-status-bar span:nth-child(2)",
        )?.textContent,
        layout: layout === undefined ? null : { height: layout.height, width: layout.width },
      };
    });
    throw new Error(`visible writer did not resize target geometry: ${JSON.stringify(diagnostic)}`);
  }
  await page.setViewportSize({ width: 980, height: 720 });

  await page.getByRole("button", { name: "Detach" }).click();
  if ((await page.locator(".workspace-tabs > .workspace-tab").count()) !== 2) {
    throw new Error("detaching the active workspace did not close only its page-memory tab");
  }
  await page.getByRole("button", { name: "Take control" }).waitFor();
  await page.getByRole("button", { name: "Take control" }).click();
  await page
    .locator(".workspace-slot.is-active .control-state")
    .getByText("You have control", { exact: true })
    .waitFor();
  await page.waitForFunction(
    (marker) =>
      [...document.querySelectorAll(".workspace-slot.is-active .xterm-rows")].some((rows) =>
        rows.textContent?.includes(String(marker)),
      ),
    firstMarker,
  );

  const takeoverPage = await browser.newPage({ viewport: { width: 390, height: 844 } });
  takeoverPage.on("console", (message) => {
    if (message.type() === "error") browserErrors.push(message.text());
  });
  takeoverPage.on("pageerror", (error) => browserErrors.push(error.message));
  await takeoverPage.goto(server, { waitUntil: "networkidle" });
  await takeoverPage.getByLabel("Deployment API key").fill(apiKey);
  await takeoverPage.getByRole("button", { name: "Open OwlMux" }).click();
  const takeoverCard = takeoverPage.locator(".host-card").filter({ hasText: "e2e-target" });
  await takeoverCard.waitFor();
  await takeoverCard.getByRole("button", { name: "Open" }).click();
  await takeoverPage.getByRole("heading", { name: "Choose a tmux session" }).waitFor();
  await takeoverPage.getByRole("button", { name: "Take over" }).click();
  await takeoverPage.waitForFunction(
    () => document.querySelector(".control-state")?.textContent === "You have control",
  );
  await page.locator(".control-state").getByText("View only · controlled elsewhere").waitFor();
  await page.waitForFunction(
    (marker) =>
      [...document.querySelectorAll(".xterm-rows")].some((rows) =>
        rows.textContent?.includes(String(marker)),
      ),
    firstMarker,
  );

  await takeoverPage
    .locator(".session-row")
    .filter({ hasText: "alpha" })
    .getByRole("button", { name: "Open", exact: true })
    .click();
  await takeoverPage.locator('[aria-label^="Writable terminal"]').waitFor();
  await waitForStableGrid(takeoverPage);
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
  for (let index = 0; index < 16; index += 1) {
    await page
      .locator(".primary-navigation")
      .getByRole("button", { name: "Hosts", exact: true })
      .click();
    await page
      .getByRole("listitem")
      .filter({ hasText: "e2e-target" })
      .getByRole("button", { name: "Open" })
      .click();
    await page.getByRole("heading", { name: "Choose a tmux session" }).waitFor();
  }
  if ((await page.locator(".workspace-tabs > .workspace-tab").count()) !== 17) {
    throw new Error("the page did not retain exactly 16 bounded workspace tabs");
  }
  await page
    .locator(".primary-navigation")
    .getByRole("button", { name: "Hosts", exact: true })
    .click();
  await page
    .getByRole("listitem")
    .filter({ hasText: "e2e-target" })
    .getByRole("button", { name: "Open" })
    .click();
  await page
    .getByText("This page already has 16 workspace tabs. Close one before opening another Host.")
    .waitFor();
  if (!page.url().endsWith("/hosts")) {
    throw new Error("the seventeenth workspace attempt escaped the bounded Host page");
  }

  await page.getByRole("button", { name: "Log out", exact: true }).click();
  await page.getByLabel("Deployment API key").waitFor();
  if (!page.url().endsWith("/login")) throw new Error("logout did not return to /login");
  await page.getByLabel("Deployment API key").fill(apiKey);
  await page.getByRole("button", { name: "Open OwlMux" }).click();
  await page.getByText("e2e-target", { exact: true }).waitFor();
  await page.reload({ waitUntil: "networkidle" });
  await page.getByLabel("Deployment API key").waitFor();
  if (await page.getByText("e2e-target", { exact: true }).isVisible()) {
    throw new Error("reload retained Browser authentication state");
  }
  console.log(
    "Chromium CSP, safe text, Hosts navigation and rename, page-memory search/logout/tabs and 16-tab bound, mutation ambiguity reconciliation, automatic target-authoritative resize, xterm geometry, same-page and mobile writer takeover without renderer rollback, and reload-memory behavior verified",
  );
} finally {
  await browser.close();
}
