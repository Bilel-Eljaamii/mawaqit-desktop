/**
 * Hostile tests for the per-prayer notification settings helpers. The
 * alerts block reaches the UI over IPC from a config file any process
 * running as the user can write — every value here is attacker-controlled
 * and must land as a sane setting or a default, never as a crash or a
 * broken panel.
 */
import { describe, expect, it } from "vitest";
import {
  MAX_NOTIFY_BEFORE_MIN,
  NOTIFY_LADDER,
  PRAYER_ALERT_KEYS,
  applyToAll,
  clampVolume,
  defaultAlerts,
  normalizeAlerts,
  notifyBeforeLabel,
  seedAlertsFromSound,
  stepNotifyBefore,
} from "../../src/lib/notifications";

// ---------------------------------------------------------------- defaults

describe("defaults and legacy seeding", () => {
  it("defaults to the historical behavior: adhan everywhere, no extras", () => {
    const alerts = defaultAlerts();
    for (const key of PRAYER_ALERT_KEYS) {
      expect(alerts[key]).toEqual({
        mode: "adhan",
        sound: null,
        volume: null,
        notify_before_min: null,
      });
    }
    expect(alerts.shuruq_notify_before_min).toBeNull();
  });

  it("seeds all five prayers from the legacy global sound switch", () => {
    const on = seedAlertsFromSound(true);
    for (const key of PRAYER_ALERT_KEYS) expect(on[key].mode).toBe("adhan");

    const off = seedAlertsFromSound(false);
    for (const key of PRAYER_ALERT_KEYS) expect(off[key].mode).toBe("silent");
  });
});

// -------------------------------------------------------------- normalize

describe("normalizeAlerts with hostile payloads", () => {
  it("falls back to the legacy seed for non-object payloads", () => {
    for (const raw of [undefined, null, 42, "silent", [], true]) {
      const alerts = normalizeAlerts(raw, false);
      expect(alerts).toEqual(seedAlertsFromSound(false));
    }
    // Same garbage with the legacy switch on seeds adhan mode instead.
    expect(normalizeAlerts("junk", true)).toEqual(seedAlertsFromSound(true));
  });

  it("rejects unusable prayer blocks but keeps usable siblings", () => {
    const alerts = normalizeAlerts(
      {
        fajr: "adhan", // string, not an object -> seeded
        dhuhr: { mode: "LOUD" }, // bad enum -> mode falls back to adhan
        asr: { mode: "default" }, // valid
      },
      false,
    );
    expect(alerts.fajr.mode).toBe("silent"); // inherited from sound_enabled=false
    expect(alerts.dhuhr.mode).toBe("adhan"); // fallback default
    expect(alerts.asr.mode).toBe("default");
    expect(alerts.maghrib.mode).toBe("silent");
  });

  it("clamps hostile numbers instead of crashing or amplifying", () => {
    const alerts = normalizeAlerts(
      {
        fajr: { volume: 9999, notify_before_min: 999_999 },
        dhuhr: { volume: -5, notify_before_min: 0 },
        asr: { volume: "50", notify_before_min: "5" }, // wrong types
      },
      true,
    );
    expect(alerts.fajr.volume).toBe(100);
    expect(alerts.fajr.notify_before_min).toBe(MAX_NOTIFY_BEFORE_MIN);
    expect(alerts.dhuhr.volume).toBe(0);
    expect(alerts.dhuhr.notify_before_min).toBeNull(); // 0 = off
    expect(alerts.asr.volume).toBeNull();
    expect(alerts.asr.notify_before_min).toBeNull();
  });

  it("keeps hostile sound paths lossless as text", () => {
    // The path is rendered with textContent later; the normalizer must not
    // mangle it (inert text in, inert text out).
    const hostile = '"><script>alert(1)</script>\u202Eesrever';
    const alerts = normalizeAlerts({ fajr: { sound: hostile } }, true);
    expect(alerts.fajr.sound).toBe(hostile);
  });

  it("drops removed alert fields instead of choking on them", () => {
    // `live_timer` existed in the brief v0.2.0 era and was removed: old
    // configs still carry it and must load cleanly with it dropped.
    const alerts = normalizeAlerts(
      { fajr: { mode: "default", notify_before_min: 5, live_timer: true } },
      true,
    );
    expect(alerts.fajr).toEqual({
      mode: "default",
      sound: null,
      volume: null,
      notify_before_min: 5,
    });
    expect("live_timer" in alerts.fajr).toBe(false);
  });

  it("normalizes the shuruq reminder independently", () => {
    expect(normalizeAlerts({ shuruq_notify_before_min: 15 }, true).shuruq_notify_before_min).toBe(15);
    expect(normalizeAlerts({ shuruq_notify_before_min: null }, true).shuruq_notify_before_min).toBeNull();
    expect(normalizeAlerts({ shuruq_notify_before_min: 1e9 }, true).shuruq_notify_before_min).toBe(
      MAX_NOTIFY_BEFORE_MIN,
    );
    expect(normalizeAlerts({}, true).shuruq_notify_before_min).toBeNull();
  });
});

// ----------------------------------------------------------------- volume

describe("clampVolume", () => {
  it("clamps into 0–100 as an integer", () => {
    expect(clampVolume(50)).toBe(50);
    expect(clampVolume(0)).toBe(0);
    expect(clampVolume(100)).toBe(100);
    expect(clampVolume(255)).toBe(100);
    expect(clampVolume(-3)).toBe(0);
    expect(clampVolume(49.6)).toBe(50);
  });

  it("survives non-numbers", () => {
    expect(clampVolume(Number.NaN)).toBe(0);
    expect(clampVolume(Number.POSITIVE_INFINITY)).toBe(0);
    expect(clampVolume(Number.NEGATIVE_INFINITY)).toBe(0);
  });
});

// ---------------------------------------------------------------- stepper

describe("stepNotifyBefore", () => {
  it("walks the ladder from None upward and back", () => {
    expect(stepNotifyBefore(null, 1)).toBe(1);
    expect(stepNotifyBefore(1, 1)).toBe(2);
    expect(stepNotifyBefore(3, 1)).toBe(5);
    expect(stepNotifyBefore(5, -1)).toBe(3);
    expect(stepNotifyBefore(1, -1)).toBeNull();
    expect(stepNotifyBefore(null, -1)).toBeNull();
  });

  it("clamps at both ends of the ladder", () => {
    expect(stepNotifyBefore(30, 1)).toBe(30);
    let v: number | null = null;
    for (let i = 0; i < NOTIFY_LADDER.length + 5; i++) v = stepNotifyBefore(v, 1);
    expect(v).toBe(30);
  });

  it("snaps hostile off-ladder values onto the ladder without jumping up", () => {
    // A tampered config set 47 minutes: stepping must land on a sane step.
    expect(stepNotifyBefore(47, -1)).toBe(30);
    expect(stepNotifyBefore(47, 1)).toBe(30);
    expect(stepNotifyBefore(1000, -1)).toBe(30);
    expect(stepNotifyBefore(4, -1)).toBe(3);
    expect(stepNotifyBefore(4, 1)).toBe(5);
  });
});

// ------------------------------------------------------------------ label

describe("notifyBeforeLabel", () => {
  it("labels None, singular and plural", () => {
    expect(notifyBeforeLabel(null)).toBe("None");
    expect(notifyBeforeLabel(1)).toBe("1 Minute");
    expect(notifyBeforeLabel(5)).toBe("5 Minutes");
  });
});

// --------------------------------------------------------------- apply all

describe("applyToAll", () => {
  it("deep-copies the current prayer onto the other four", () => {
    const alerts = defaultAlerts();
    alerts.fajr = { mode: "silent", sound: "/tmp/a.mp3", volume: 40, notify_before_min: 5 };
    const next = applyToAll(alerts, "fajr");

    for (const key of PRAYER_ALERT_KEYS) {
      expect(next[key]).toEqual(alerts.fajr);
    }
    // Global shuruq setting is untouched.
    expect(next.shuruq_notify_before_min).toBe(alerts.shuruq_notify_before_min);
  });

  it("never mutates the input and never shares nested objects", () => {
    const alerts = defaultAlerts();
    alerts.dhuhr.notify_before_min = 10;
    const next = applyToAll(alerts, "dhuhr");

    next.asr.notify_before_min = 30;
    expect(alerts.asr.notify_before_min).toBeNull();
    expect(next.dhuhr).not.toBe(alerts.dhuhr);
  });
});
