// Vitest setup. The frontend logic modules are deliberately free of Tauri IPC so
// they can be unit-tested in jsdom without a running backend. Components that call
// invoke() import from lib/ipc, which is mocked per-test where needed.
import { vi } from "vitest";

// Provide a no-op __TAURI_INTERNALS__ so accidental imports of the Tauri API in a
// component under test don't throw. Pure-logic tests never touch this.
(globalThis as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
  invoke: vi.fn(async () => undefined),
  transformCallback: vi.fn(),
};
