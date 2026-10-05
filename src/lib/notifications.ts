// Pure per-prayer notification settings helpers, extracted from main.ts so
// the test suite can hammer them without a Tauri runtime. The alerts block
// mirrors the Rust `AlertsConfig` serde structs (src-tauri
// src/domain/models.rs) — keep both in sync. Although the backend already
// normalizes the config it serves, every field is treated as
// attacker-influenced: the config file is writable by any process running as
// the user, so `normalizeAlerts` re-validates everything at the UI boundary.

// ---- Types mirroring the Rust serde structs ----

export type AthanMode = "silent" | "default" | "adhan";

export interface PrayerAlerts {
  mode: AthanMode;
  /** Catalog voice id (mawaqit-api ADHAN_VOICES); null = builtin/custom. */
  voice: string | null;
  /** null = built-in athan; otherwise an absolute path to an mp3/wav file. */
  sound: string | null;
  /** 0–100 percent, or null to follow the system volume. */
  volume: number | null;
  /** Minutes before the adhan for a heads-up notification, or null = off.
   * The heads-up fires exactly once per prayer and day. */
  notify_before_min: number | null;
}

export interface AlertsConfig {
  fajr: PrayerAlerts;
  dhuhr: PrayerAlerts;
  asr: PrayerAlerts;
  maghrib: PrayerAlerts;
  isha: PrayerAlerts;
  shuruq_notify_before_min: number | null;
}

// ---- Constants ----

/** The prayer keys, in the order the Rust `PRAYERS` table uses. */
export const PRAYER_ALERT_KEYS = ["fajr", "dhuhr", "asr", "maghrib", "isha"] as const;
export type PrayerKey = (typeof PRAYER_ALERT_KEYS)[number];

/** "Notify before" stepper: None, then the useful jumps (matches the
 * Mawaqit mobile app's +/− ladder). */
export const NOTIFY_LADDER: (number | null)[] = [null, 1, 2, 3, 5, 10, 15, 20, 30];

/** Hard cap the backend also enforces (prayer_logic::MAX_NOTIFY_BEFORE_MIN). */
export const MAX_NOTIFY_BEFORE_MIN = 120;

export const MODE_LABELS: Record<AthanMode, string> = {
  silent: "Silent",
  default: "Default",
  adhan: "Adhan",
};

// ---- Factories & coercion ----

export function defaultPrayerAlerts(): PrayerAlerts {
  return { mode: "adhan", voice: null, sound: null, volume: null, notify_before_min: null };
}

export function defaultAlerts(): AlertsConfig {
  return {
    fajr: defaultPrayerAlerts(),
    dhuhr: defaultPrayerAlerts(),
    asr: defaultPrayerAlerts(),
    maghrib: defaultPrayerAlerts(),
    isha: defaultPrayerAlerts(),
    shuruq_notify_before_min: null,
  };
}

/** Upgrade migration: a config from before per-prayer settings existed
 * seeds every prayer from the legacy global sound switch. */
export function seedAlertsFromSound(soundEnabled: boolean): AlertsConfig {
  const alerts = defaultAlerts();
  const mode: AthanMode = soundEnabled ? "adhan" : "silent";
  for (const key of PRAYER_ALERT_KEYS) alerts[key].mode = mode;
  return alerts;
}

/** Coerce anything (an IPC payload typed as AlertsConfig can still carry
 * garbage from a tampered config file) into a sane AlertsConfig. Missing or
 * unusable prayers inherit the legacy `sound_enabled` switch; usable
 * per-prayer values win. Never throws. */
export function normalizeAlerts(raw: unknown, soundEnabled = true): AlertsConfig {
  const seeded = seedAlertsFromSound(soundEnabled);
  if (typeof raw !== "object" || raw === null) return seeded;
  const record = raw as Record<string, unknown>;
  for (const key of PRAYER_ALERT_KEYS) {
    const parsed = normalizePrayerAlerts(record[key]);
    if (parsed !== null) seeded[key] = parsed;
  }
  const shuruq = normalizeNotifyBefore(record["shuruq_notify_before_min"]);
  if (shuruq !== undefined) seeded.shuruq_notify_before_min = shuruq;
  return seeded;
}

/** A single prayer's block, or null when it is absent/unusable. */
function normalizePrayerAlerts(raw: unknown): PrayerAlerts | null {
  if (typeof raw !== "object" || raw === null) return null;
  const r = raw as Record<string, unknown>;

  const mode = r["mode"];
  const saneMode: AthanMode | null =
    mode === "silent" || mode === "default" || mode === "adhan" ? mode : null;

  const saneSound =
    typeof r["sound"] === "string" && (r["sound"] as string).trim() !== ""
      ? (r["sound"] as string)
      : null;

  const saneVolume =
    typeof r["volume"] === "number" && Number.isFinite(r["volume"])
      ? clampVolume(r["volume"] as number)
      : null;

  const before = normalizeNotifyBefore(r["notify_before_min"]);
  const saneBefore = before === undefined ? null : before;

  return {
    mode: saneMode ?? "adhan",
    sound: saneSound,
    volume: saneVolume,
    notify_before_min: saneBefore,
    voice: null,
  };
}

/** null stays null (off); a usable number clamps to [1, MAX]; anything else
 * is undefined (caller decides the fallback). */
function normalizeNotifyBefore(raw: unknown): number | null | undefined {
  if (raw === null) return null;
  if (typeof raw === "number" && Number.isFinite(raw) && raw > 0) {
    return Math.min(Math.round(raw), MAX_NOTIFY_BEFORE_MIN);
  }
  return undefined;
}

/** 0–100 integer volume; NaN/Infinity/out-of-range all clamp. */
export function clampVolume(v: number): number {
  if (!Number.isFinite(v)) return 0;
  return Math.min(100, Math.max(0, Math.round(v)));
}

// ---- Stepper ----

/** One +/- press on the "Notify before" stepper, walking NOTIFY_LADDER.
 * Values not on the ladder (e.g. a hostile 47 from the config) snap to the
 * nearest step going down, then keep stepping. */
export function stepNotifyBefore(current: number | null, direction: 1 | -1): number | null {
  const index = NOTIFY_LADDER.indexOf(current);
  if (index === -1) {
    // Off-ladder value: find the first step strictly below (direction -1) or
    // strictly above (+1) it.
    const comparable = current ?? -1;
    if (direction === -1) {
      const below = NOTIFY_LADDER.filter((v) => v !== null && v < comparable);
      return below.length > 0 ? (below[below.length - 1] as number) : null;
    }
    const above = NOTIFY_LADDER.filter((v) => v !== null && v > comparable);
    return above.length > 0 ? (above[0] as number) : NOTIFY_LADDER[NOTIFY_LADDER.length - 1];
  }
  const next = index + direction;
  return NOTIFY_LADDER[Math.min(NOTIFY_LADDER.length - 1, Math.max(0, next))] ?? null;
}

export function notifyBeforeLabel(n: number | null): string {
  return n === null ? "None" : n === 1 ? "1 Minute" : `${n} Minutes`;
}

// ---- Bulk edit ----

/** "Set this for every prayer": deep-copy `from`'s settings onto the other
 * four prayers. Returns a new object; the input is never mutated. */
export function applyToAll(alerts: AlertsConfig, from: PrayerKey): AlertsConfig {
  const source = alerts[from] ?? defaultPrayerAlerts();
  const copy = (): PrayerAlerts => ({ ...source });
  const next: AlertsConfig = { ...alerts, shuruq_notify_before_min: alerts.shuruq_notify_before_min };
  for (const key of PRAYER_ALERT_KEYS) next[key] = copy();
  return next;
}
