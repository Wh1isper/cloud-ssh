export const API_KEY_STORAGE_KEY = "owlmux.deployment_api_key.v1";
export const API_KEY_LENGTH = 56;

const API_KEY_PATTERN = /^owlmux_sk_v1_[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$/;

export type StoredApiKeyResult =
  | { status: "absent" }
  | { apiKey: string; status: "available" }
  | { removalFailed: boolean; status: "invalid" }
  | { status: "unavailable" };

function browserStorage(): Storage | null {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

export function isValidApiKey(apiKey: string): boolean {
  return apiKey.length === API_KEY_LENGTH && API_KEY_PATTERN.test(apiKey);
}

export function readStoredApiKey(storage: Storage | null = browserStorage()): StoredApiKeyResult {
  if (storage === null) return { status: "unavailable" };
  try {
    const apiKey = storage.getItem(API_KEY_STORAGE_KEY);
    if (apiKey === null) return { status: "absent" };
    if (isValidApiKey(apiKey)) return { apiKey, status: "available" };
    return { removalFailed: !clearStoredApiKey(storage), status: "invalid" };
  } catch {
    return { status: "unavailable" };
  }
}

export function storeApiKey(apiKey: string, storage: Storage | null = browserStorage()): boolean {
  if (storage === null || !isValidApiKey(apiKey)) return false;
  try {
    storage.setItem(API_KEY_STORAGE_KEY, apiKey);
    return true;
  } catch {
    return false;
  }
}

export function clearStoredApiKey(storage: Storage | null = browserStorage()): boolean {
  if (storage === null) return false;
  try {
    storage.removeItem(API_KEY_STORAGE_KEY);
    return true;
  } catch {
    return false;
  }
}
