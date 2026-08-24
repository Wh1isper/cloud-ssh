import { describe, expect, it, vi } from "vitest";

import {
  API_KEY_LENGTH,
  API_KEY_STORAGE_KEY,
  clearStoredApiKey,
  isValidApiKey,
  readStoredApiKey,
  storeApiKey,
} from "./auth-storage";

const VALID_API_KEY = "owlmux_sk_v1_YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE";

function memoryStorage(): Storage {
  const values = new Map<string, string>();
  return {
    get length() {
      return values.size;
    },
    clear: () => values.clear(),
    getItem: (key) => values.get(key) ?? null,
    key: (index) => [...values.keys()][index] ?? null,
    removeItem: (key) => void values.delete(key),
    setItem: (key, value) => void values.set(key, value),
  };
}

describe("Deployment API key storage", () => {
  it("accepts only the exact canonical API key shape", () => {
    expect(VALID_API_KEY).toHaveLength(API_KEY_LENGTH);
    expect(isValidApiKey(VALID_API_KEY)).toBe(true);
    expect(isValidApiKey(` ${VALID_API_KEY}`)).toBe(false);
    expect(isValidApiKey(VALID_API_KEY.slice(0, -1))).toBe(false);
    expect(isValidApiKey(`${VALID_API_KEY.slice(0, -1)}B`)).toBe(false);
    expect(isValidApiKey(`owlmux_sk_v2_${VALID_API_KEY.slice(13)}`)).toBe(false);
  });

  it("round-trips one same-origin key and clears it", () => {
    const storage = memoryStorage();

    expect(readStoredApiKey(storage)).toEqual({ status: "absent" });
    expect(storeApiKey(VALID_API_KEY, storage)).toBe(true);
    expect(storage.getItem(API_KEY_STORAGE_KEY)).toBe(VALID_API_KEY);
    expect(readStoredApiKey(storage)).toEqual({ apiKey: VALID_API_KEY, status: "available" });

    expect(clearStoredApiKey(storage)).toBe(true);
    expect(readStoredApiKey(storage)).toEqual({ status: "absent" });
  });

  it("removes malformed stored values before returning them", () => {
    const storage = memoryStorage();
    storage.setItem(API_KEY_STORAGE_KEY, "owlmux_sk_v1_not-a-key");

    expect(readStoredApiKey(storage)).toEqual({ removalFailed: false, status: "invalid" });
    expect(storage.getItem(API_KEY_STORAGE_KEY)).toBeNull();
    expect(storeApiKey("owlmux_sk_v1_not-a-key", storage)).toBe(false);
  });

  it("reports storage access and malformed-value removal failures", () => {
    const inaccessible = {
      getItem: vi.fn(() => {
        throw new DOMException("blocked");
      }),
      removeItem: vi.fn(() => {
        throw new DOMException("blocked");
      }),
      setItem: vi.fn(() => {
        throw new DOMException("blocked");
      }),
    } as unknown as Storage;
    expect(readStoredApiKey(inaccessible)).toEqual({ status: "unavailable" });
    expect(storeApiKey(VALID_API_KEY, inaccessible)).toBe(false);
    expect(clearStoredApiKey(inaccessible)).toBe(false);

    const malformed = memoryStorage();
    malformed.setItem(API_KEY_STORAGE_KEY, "bad");
    malformed.removeItem = vi.fn(() => {
      throw new DOMException("blocked");
    });
    expect(readStoredApiKey(malformed)).toEqual({ removalFailed: true, status: "invalid" });
  });
});
