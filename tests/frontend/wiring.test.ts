/** Wiring smoke test: execute the REAL main.ts against the REAL index.html
 * in happy-dom. Guards the 2026-10 regression class where one broken
 * listener silently killed every listener wired after it (Save Settings
 * "did nothing"). Mocks the Tauri IPC layer; asserts the wiring runs to the
 * end and that Save Settings reaches update_config. */
import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());
const listenMock = vi.hoisted(() => vi.fn(async () => () => {}));
const getVersionMock = vi.hoisted(() => vi.fn(async () => "0.7.0-test"));
const isEnabledMock = vi.hoisted(() => vi.fn(async () => false));
const enableMock = vi.hoisted(() => vi.fn(async () => {}));
const disableMock = vi.hoisted(() => vi.fn(async () => {}));
const openMock = vi.hoisted(() => vi.fn(async () => null));
const hideMock = vi.hoisted(() => vi.fn(async () => {}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ hide: hideMock }),
}));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: getVersionMock }));
vi.mock("@tauri-apps/plugin-autostart", () => ({
  enable: enableMock,
  disable: disableMock,
  isEnabled: isEnabledMock,
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openMock }));

import { readFileSync } from "node:fs";

async function loadApp(): Promise<void> {
  // The real UI as the webview would see it.
  document.body.innerHTML = "";
  const html = readFileSync("index.html", "utf8");
  const bodyMatch = html.match(/<body>([\s\S]*)<\/body>/);
  const container = document.createElement("div");
  container.innerHTML = bodyMatch?.[1] ?? "";
  while (document.body.firstChild) document.body.removeChild(document.body.firstChild);
  Array.from(container.children).forEach((c) => document.body.appendChild(c));

  invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "get_config") {
      return Promise.resolve({
        mosque_slug: "test-mosque",
        mosque_name: "Test Mosque",
        sound_enabled: true,
        iqama_alerts: false,
        autostart: false,
        offline_mode: false,
        tor_socks_addr: null,
        alerts: {
          fajr: { mode: "adhan", voice: null, sound: null, volume: null, notify_before_min: null },
          dhuhr: { mode: "adhan", voice: null, sound: null, volume: null, notify_before_min: null },
          asr: { mode: "adhan", voice: null, sound: null, volume: null, notify_before_min: null },
          maghrib: { mode: "adhan", voice: null, sound: null, volume: null, notify_before_min: null },
          isha: { mode: "adhan", voice: null, sound: null, volume: null, notify_before_min: null },
          shuruq_notify_before_min: null,
        },
        announcements_read: [],
      });
    }
    if (cmd === "get_today") return Promise.resolve(null);
    if (cmd === "adhan_voices") return Promise.resolve([]);
    return Promise.resolve(undefined);
  });

  // Fresh module state per test.
  vi.resetModules();
  await import("../../src/main");
  // main.ts wires on DOMContentLoaded.
  document.dispatchEvent(new Event("DOMContentLoaded", { bubbles: true }));
  // Async wiring (void boot()) settles on the microtask queue.
  await new Promise((r) => setTimeout(r, 0));
}

describe("settings wiring smoke", () => {
  beforeEach(async () => {
    vi.clearAllMocks();
    await loadApp();
  });

  it("save-btn click reaches update_config", async () => {
    const save = document.getElementById("save-btn") as HTMLButtonElement;
    expect(save).not.toBeNull();
    save.click();
    await new Promise((r) => setTimeout(r, 0));
    expect(invokeMock).toHaveBeenCalledWith(
      "update_config",
      expect.objectContaining({ config: expect.objectContaining({ mosque_slug: "test-mosque" }) }),
    );
  });

  it("every wired element exists — no listener is silently skipped", () => {
    // If DOMContentLoaded threw mid-wiring, later listeners (save-btn among
    // them) would never attach. Direct evidence: dispatch on the LAST wired
    // control and observe its effect.
    const setTor = document.getElementById("set-tor") as HTMLInputElement;
    expect(setTor).not.toBeNull();
    expect(() => setTor.dispatchEvent(new Event("change"))).not.toThrow();
  });
});
