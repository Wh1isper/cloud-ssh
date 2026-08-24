/** @vitest-environment jsdom */

import { StrictMode, act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { API_KEY_LENGTH, API_KEY_STORAGE_KEY } from "./auth-storage";
import { App } from "./App";

vi.mock("./workspace", () => ({ InteractiveWorkspace: () => null }));

const VALID_API_KEY = "owlmux_sk_v1_YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE";
const DEPLOYMENT = {
  config_epoch: 1,
  deployment_id: "00000000-0000-0000-0000-000000000001",
  profile: "single_node",
  server_build_id: "test-build",
};

let container: HTMLDivElement;
let root: Root | null;

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

function jsonResponse(value: unknown): Response {
  return new Response(JSON.stringify(value), {
    headers: { "Content-Type": "application/json" },
    status: 200,
  });
}

function installSuccessfulFetch(): ReturnType<typeof vi.fn> {
  const fetchMock = vi.fn((input: URL | RequestInfo) => {
    const pathname = new URL(String(input), window.location.origin).pathname;
    if (pathname === "/api/v1/deployment") return Promise.resolve(jsonResponse(DEPLOYMENT));
    if (
      pathname === "/api/v1/audit-events" ||
      pathname === "/api/v1/ssh-credentials" ||
      pathname === "/api/v1/machines"
    ) {
      return Promise.resolve(jsonResponse([]));
    }
    return Promise.resolve(new Response(null, { status: 404 }));
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

async function mountApp(): Promise<void> {
  await act(async () => {
    root = createRoot(container);
    root.render(
      <StrictMode>
        <App />
      </StrictMode>,
    );
  });
}

async function nextTask(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => window.setTimeout(resolve, 0));
  });
}

async function waitForText(text: string): Promise<void> {
  for (let attempt = 0; attempt < 30; attempt += 1) {
    if (container.textContent?.includes(text)) return;
    await nextTask();
  }
  throw new Error(`Timed out waiting for text: ${text}\n${container.textContent ?? ""}`);
}

function enterApiKey(apiKey = VALID_API_KEY): HTMLInputElement {
  const input = container.querySelector<HTMLInputElement>("#api-key");
  if (input === null) throw new Error("API key input is missing");
  const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  if (setValue === undefined) throw new Error("HTML input value setter is missing");
  act(() => {
    setValue.call(input, apiKey);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  return input;
}

async function submitLogin(): Promise<void> {
  const form = container.querySelector<HTMLFormElement>(".login-form");
  if (form === null) throw new Error("Login form is missing");
  await act(async () => {
    form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await Promise.resolve();
  });
}

beforeEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  window.localStorage.clear();
  window.history.replaceState(null, "", "/login");
  document.body.innerHTML = "";
  container = document.createElement("div");
  document.body.append(container);
  root = null;
});

afterEach(async () => {
  if (root !== null) {
    await act(async () => root?.unmount());
  }
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  window.localStorage.clear();
});

describe("App authentication lifecycle", () => {
  it("opens the bounded Deployment access form when no key is saved", async () => {
    await mountApp();
    await waitForText("Deployment API key");

    const input = container.querySelector<HTMLInputElement>("#api-key");
    expect(container.textContent).toContain("Your terminal sessions stay on the target.");
    expect(container.textContent).toContain("Browser storage");
    expect(input?.type).toBe("password");
    expect(input?.maxLength).toBe(API_KEY_LENGTH);
    expect(container.querySelector('img[src="/favicon.svg"]')).not.toBeNull();
  });

  it("reports unavailable Browser storage without blocking the login form", async () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new DOMException("blocked");
    });

    await mountApp();
    await waitForText("this browser blocked local storage");

    expect(container.textContent).toContain("Deployment API key");
  });

  it("removes a malformed saved key without sending it", async () => {
    window.localStorage.setItem(API_KEY_STORAGE_KEY, "owlmux_sk_v1_not-a-key");
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);

    await mountApp();
    await waitForText("Deployment API key");

    expect(window.localStorage.getItem(API_KEY_STORAGE_KEY)).toBeNull();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("warns when a malformed saved key cannot be removed", async () => {
    window.localStorage.setItem(API_KEY_STORAGE_KEY, "owlmux_sk_v1_not-a-key");
    vi.spyOn(Storage.prototype, "removeItem").mockImplementation(() => {
      throw new DOMException("blocked");
    });
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);

    await mountApp();
    await waitForText("could not remove a malformed saved API key");

    expect(window.localStorage.getItem(API_KEY_STORAGE_KEY)).toBe("owlmux_sk_v1_not-a-key");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("starts only one validation for duplicate login submission", async () => {
    let resolveValidation: ((response: Response) => void) | null = null;
    const fetchMock = vi.fn(
      () =>
        new Promise<Response>((resolve) => {
          resolveValidation = resolve;
        }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await mountApp();
    await waitForText("Deployment API key");
    enterApiKey();
    await submitLogin();
    await submitLogin();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    await act(async () => {
      const resolve = resolveValidation as ((response: Response) => void) | null;
      resolve?.(new Response(null, { status: 401 }));
      await Promise.resolve();
    });
    await waitForText("Authentication failed");
  });

  it("restores a valid key without rewriting it, preserves a deep link, and clears every copy on logout", async () => {
    window.localStorage.setItem(API_KEY_STORAGE_KEY, VALID_API_KEY);
    const setItem = vi.spyOn(Storage.prototype, "setItem");
    window.history.replaceState(null, "", "/hosts");
    installSuccessfulFetch();

    await mountApp();
    await waitForText("Log out");

    expect(window.location.pathname).toBe("/hosts");
    expect(window.localStorage.getItem(API_KEY_STORAGE_KEY)).toBe(VALID_API_KEY);
    expect(setItem).not.toHaveBeenCalled();

    const logout = [...container.querySelectorAll<HTMLButtonElement>("button")].find(
      (button) => button.textContent?.trim() === "Log out",
    );
    expect(logout).toBeDefined();
    await act(async () => logout?.click());
    await waitForText("Deployment API key");

    expect(window.location.pathname).toBe("/login");
    expect(window.localStorage.getItem(API_KEY_STORAGE_KEY)).toBeNull();
    expect(container.querySelector<HTMLInputElement>("#api-key")?.value).toBe("");
  });

  it("clears active page authority after an authenticated refresh receives HTTP 401", async () => {
    window.localStorage.setItem(API_KEY_STORAGE_KEY, VALID_API_KEY);
    let deploymentRequests = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn((input: URL | RequestInfo) => {
        const pathname = new URL(String(input), window.location.origin).pathname;
        if (pathname === "/api/v1/deployment") {
          deploymentRequests += 1;
          return Promise.resolve(
            deploymentRequests === 1
              ? jsonResponse(DEPLOYMENT)
              : new Response(null, { status: 401 }),
          );
        }
        return Promise.resolve(jsonResponse([]));
      }),
    );

    await mountApp();
    await waitForText("Authentication failed. Enter the current Deployment API key.");

    expect(window.localStorage.getItem(API_KEY_STORAGE_KEY)).toBeNull();
    expect(window.location.pathname).toBe("/login");
    expect(container.querySelector<HTMLInputElement>("#api-key")?.value).toBe("");
  });

  it("clears a saved key after restore authentication failure", async () => {
    window.localStorage.setItem(API_KEY_STORAGE_KEY, VALID_API_KEY);
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(new Response(null, { status: 401 }))),
    );

    await mountApp();
    await waitForText("The saved API key is no longer valid");

    expect(window.localStorage.getItem(API_KEY_STORAGE_KEY)).toBeNull();
    expect(container.querySelector<HTMLInputElement>("#api-key")?.value).toBe("");
  });

  it("retains a saved key for an explicit retry after a transport failure", async () => {
    window.localStorage.setItem(API_KEY_STORAGE_KEY, VALID_API_KEY);
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new TypeError("offline"))),
    );

    await mountApp();
    await waitForText("Your saved key is still available");

    expect(window.localStorage.getItem(API_KEY_STORAGE_KEY)).toBe(VALID_API_KEY);
    expect(container.querySelector<HTMLInputElement>("#api-key")?.value).toBe(VALID_API_KEY);
    expect(container.querySelector<HTMLInputElement>("#api-key")?.type).toBe("password");
  });

  it("bounds Deployment verification and retains the key after timeout", async () => {
    vi.useFakeTimers();
    window.localStorage.setItem(API_KEY_STORAGE_KEY, VALID_API_KEY);
    vi.stubGlobal(
      "fetch",
      vi.fn(() => new Promise<Response>(() => undefined)),
    );

    await mountApp();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10_000);
    });

    expect(container.textContent).toContain("Deployment verification timed out");
    expect(window.localStorage.getItem(API_KEY_STORAGE_KEY)).toBe(VALID_API_KEY);
    expect(container.querySelector<HTMLInputElement>("#api-key")?.value).toBe(VALID_API_KEY);
  });

  it("invalidates a pending login before a late pagehide response can persist access", async () => {
    let resolveValidation: ((response: Response) => void) | null = null;
    vi.stubGlobal(
      "fetch",
      vi.fn(
        () =>
          new Promise<Response>((resolve) => {
            resolveValidation = resolve;
          }),
      ),
    );

    await mountApp();
    await waitForText("Deployment API key");
    enterApiKey();
    await submitLogin();
    expect(resolveValidation).not.toBeNull();

    act(() => window.dispatchEvent(new PageTransitionEvent("pagehide")));
    await act(async () => {
      const resolve = resolveValidation as ((response: Response) => void) | null;
      resolve?.(jsonResponse(DEPLOYMENT));
      await Promise.resolve();
    });

    expect(window.localStorage.getItem(API_KEY_STORAGE_KEY)).toBeNull();
    expect(window.location.pathname).toBe("/login");
    expect(
      [...container.querySelectorAll<HTMLButtonElement>("button")].some(
        (button) => button.textContent?.trim() === "Log out",
      ),
    ).toBe(false);
  });

  it("shows a persistent warning when local storage cannot save verified access", async () => {
    installSuccessfulFetch();
    await mountApp();
    await waitForText("Deployment API key");
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new DOMException("blocked");
    });

    enterApiKey();
    await submitLogin();
    await waitForText("this browser could not save the API key");

    expect(container.textContent).toContain("Log out");
    expect(window.localStorage.getItem(API_KEY_STORAGE_KEY)).toBeNull();
  });

  it("reports failed persistent cleanup while still ending the page session", async () => {
    window.localStorage.setItem(API_KEY_STORAGE_KEY, VALID_API_KEY);
    installSuccessfulFetch();
    await mountApp();
    await waitForText("Log out");
    vi.spyOn(Storage.prototype, "removeItem").mockImplementation(() => {
      throw new DOMException("blocked");
    });

    const logout = [...container.querySelectorAll<HTMLButtonElement>("button")].find(
      (button) => button.textContent?.trim() === "Log out",
    );
    await act(async () => logout?.click());
    await waitForText("could not remove the saved API key");

    expect(container.textContent).toContain("Deployment API key");
    expect(window.localStorage.getItem(API_KEY_STORAGE_KEY)).toBe(VALID_API_KEY);
    expect(window.location.pathname).toBe("/login");
  });
});
