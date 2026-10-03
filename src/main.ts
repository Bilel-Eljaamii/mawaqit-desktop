import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { enable, disable, isEnabled } from "@tauri-apps/plugin-autostart";
import { open } from "@tauri-apps/plugin-dialog";
import {
  formatCountdown,
  mosqueDisplayName,
  parseHhmmToDate,
  renderMonthTable,
  renderResults,
  sanitizeCssUrl,
} from "./lib/display";
import type { DailyIqamaTimes, DailyPrayerTimes, Mosque } from "./lib/display";
import {
  PRAYER_ALERT_KEYS,
  applyToAll,
  clampVolume,
  defaultAlerts,
  normalizeAlerts,
  notifyBeforeLabel,
  stepNotifyBefore,
} from "./lib/notifications";
import type { AlertsConfig, AthanMode, PrayerKey } from "./lib/notifications";

// ---- Types mirroring the Rust serde structs ----

interface AppConfig {
  mosque_slug: string;
  mosque_name: string | null;
  sound_enabled: boolean;
  iqama_alerts: boolean;
  autostart: boolean;
  alerts: AlertsConfig;
}

interface TodayTimes {
  date: string;
  adhan: DailyPrayerTimes;
  iqama: DailyIqamaTimes | null;
}

interface TodayPayload {
  mosque_name: string | null;
  jumua: string | null;
  jumua2: string | null;
  image: string | null;
  imsak_mode: boolean;
  times: TodayTimes;
  /** Fetch date (ISO) of the offline snapshot when served from disk. */
  as_of: string | null;
}

interface MonthTimes {
  month: number;
  days: { day: number; times: DailyPrayerTimes }[];
}

interface MonthIqamaTimes {
  month: number;
  days: { day: number; times: DailyIqamaTimes }[];
}

// ---- State & helpers ----

const $ = <T extends HTMLElement = HTMLElement>(id: string): T =>
  document.getElementById(id) as T;

let config: AppConfig | null = null;
let payload: TodayPayload | null = null;
let selectedMosque: Mosque | null = null;
let monthIndex = new Date().getMonth() + 1; // 1-12
let monthMode: "adhan" | "iqama" = "adhan";

const PRAYERS: { key: keyof DailyPrayerTimes; label: string }[] = [
  { key: "fajr", label: "Fajr" },
  { key: "dhuhr", label: "Dhuhr" },
  { key: "asr", label: "Asr" },
  { key: "maghrib", label: "Maghrib" },
  { key: "isha", label: "Isha" },
];

const IQAMA_PRAYERS = [
  { key: "fajr" as const, label: "Fajr" },
  { key: "dhuhr" as const, label: "Dhuhr" },
  { key: "asr" as const, label: "Asr" },
  { key: "maghrib" as const, label: "Maghrib" },
  { key: "isha" as const, label: "Isha" },
];

const MONTH_NAMES = [
  "January", "February", "March", "April", "May", "June",
  "July", "August", "September", "October", "November", "December",
];

const DEFAULT_CONFIG: AppConfig = {
  mosque_slug: "",
  mosque_name: null,
  sound_enabled: true,
  iqama_alerts: false,
  autostart: true,
  alerts: defaultAlerts(),
};

// ---- Prayer notifications panel state ----

let alertsDraft: AlertsConfig = defaultAlerts();
let notifyTab: PrayerKey = "fajr";

function toast(message: string, isError = true): void {
  const el = $("toast");
  el.textContent = message;
  el.classList.toggle("error", isError);
  el.hidden = false;
  window.setTimeout(() => (el.hidden = true), 4000);
}

function showView(name: "onboarding" | "today" | "month"): void {
  $("onboarding").hidden = name !== "onboarding";
  $("view-today").hidden = name !== "today";
  $("view-month").hidden = name !== "month";
  $("tabs").style.visibility = name === "onboarding" ? "hidden" : "visible";
  $("tab-today").classList.toggle("active", name === "today");
  $("tab-month").classList.toggle("active", name === "month");
}

function isConfigured(): boolean {
  return !!config && config.mosque_slug !== "";
}

// ---- Today view (mawaqit.net display style) ----

function renderToday(): void {
  const row = $("prayer-row");
  row.innerHTML = "";

  if (!payload) {
    $("mosque-name").textContent = config?.mosque_name ?? "Mawaqit";
    $("next-name").textContent = "No data";
    $("next-countdown").textContent = "--:--";
    $("custom-times").hidden = true;
    return;
  }

  $("mosque-name").textContent =
    payload.mosque_name ?? config?.mosque_name ?? "Mawaqit";
  document.title = `Mawaqit — ${$("mosque-name").textContent}`;

  // Mosque picture as the backdrop, like the mawaqit.net display page.
  // The URL goes into a CSS custom property, so it must be sanitized: only
  // http(s) URLs, no quotes/backslashes/parens (blocks CSS break-out from a
  // hostile mosque config).
  const app = $("app");
  const image = payload.image ? sanitizeCssUrl(payload.image) : null;
  if (image) {
    app.style.setProperty("--mosque-image", `url("${image}")`);
    app.classList.add("with-image");
  } else {
    app.classList.remove("with-image");
  }

  // Prayer cards: name / adhan / iqama. In "Sabah Imsak" mode (DİTİB), the
  // first prayer is labeled Imsak, like mawaqit.net does.
  for (const { key, label } of PRAYERS) {
    const card = document.createElement("div");
    card.className = "prayer";
    card.dataset.prayer = key;

    const name = document.createElement("div");
    name.className = "name";
    name.textContent =
      key === "fajr" && payload.imsak_mode ? "Imsak" : label;
    card.appendChild(name);

    const time = document.createElement("div");
    time.className = "time";
    time.textContent = payload.times.adhan[key];
    card.appendChild(time);

    const wait = document.createElement("div");
    wait.className = "wait";
    wait.textContent = payload.times.iqama
      ? `iqama ${payload.times.iqama[key as keyof DailyIqamaTimes]}`
      : " ";
    card.appendChild(wait);

    row.appendChild(card);
  }

  // Shurouq + Jumu'a extras.
  $("ct-shurouq-val").textContent = payload.times.adhan.shurouq;
  $("ct-shurouq").hidden = false;

  // Offline badge: only set when the backend served the disk snapshot.
  const offlineBadge = $("offline-badge");
  offlineBadge.hidden = !payload.as_of;
  if (payload.as_of) {
    const parsed = new Date(`${payload.as_of}T00:00`);
    const shown = Number.isNaN(parsed.getTime())
      ? payload.as_of
      : parsed.toLocaleDateString();
    offlineBadge.textContent = `Offline — times from ${shown}`;
  }

  if (payload.jumua) {
    $("ct-jumua-val").textContent = payload.jumua;
    $("ct-jumua2-val").textContent = payload.jumua2 ? ` / ${payload.jumua2}` : "";
    $("ct-jumua").hidden = false;
  } else {
    $("ct-jumua").hidden = true;
  }
  $("custom-times").hidden = false;

  tick();
}

function buildEvents(dayOffset: number): { label: string; at: Date }[] {
  if (!payload) return [];
  const fajrLabel = payload.imsak_mode ? "Imsak" : "Fajr";
  const events: { label: string; at: Date }[] = [];
  for (const { key, label } of IQAMA_PRAYERS) {
    const shown = key === "fajr" ? fajrLabel : label;
    const t = parseHhmmToDate(payload.times.adhan[key], dayOffset);
    if (t) events.push({ label: shown, at: t });
    if (config?.iqama_alerts && payload.times.iqama) {
      const iq = parseHhmmToDate(payload.times.iqama[key], dayOffset);
      if (iq) events.push({ label: `${shown} iqama`, at: iq });
    }
  }
  return events;
}

function computeNextEvent(): { label: string; at: Date } | null {
  if (!payload) return null;
  const now = new Date();

  const upcoming = buildEvents(0)
    .filter((e) => e.at.getTime() > now.getTime())
    .sort((a, b) => a.at.getTime() - b.at.getTime());
  if (upcoming.length > 0) return upcoming[0];

  // Everything passed today: first event tomorrow.
  const tomorrow = buildEvents(1).sort((a, b) => a.at.getTime() - b.at.getTime());
  return tomorrow[0] ?? null;
}

function tick(): void {
  // Clock, like the mawaqit.net display.
  const now = new Date();
  $("current-clock").textContent = now.toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });

  // While the stop button is visible, keep checking so it also disappears
  // when the athan ends on its own.
  if (!$("stop-athan-btn").hidden) refreshAthanButton();

  if (!payload) return;
  const next = computeNextEvent();
  if (!next) return;

  // Window title carries the countdown: visible in taskbar previews and
  // alt-tab without opening anything.
  document.title = `Mawaqit — ${next.label} in ${formatCountdown(next.at.getTime() - Date.now())}`;

  $("next-label").textContent = next.label.endsWith("iqama")
    ? "Next iqama"
    : "Next prayer";
  $("next-name").textContent = next.label;
  $("next-countdown").textContent = formatCountdown(next.at.getTime() - Date.now());

  // Highlight the prayer card whose adhan is next.
  const highlightKey = next.label.replace(" iqama", "").toLowerCase();
  document.querySelectorAll(".prayer").forEach((card) => {
    card.classList.toggle(
      "next",
      (card as HTMLElement).dataset.prayer === highlightKey,
    );
  });
}

// ---- Data loading ----

async function loadConfig(): Promise<void> {
  config = await invoke<AppConfig>("get_config");
  // The backend normalizes its config, but the file is attacker-writable:
  // re-validate at the UI boundary so the panel never sees garbage.
  config.alerts = normalizeAlerts(config.alerts, config.sound_enabled);
}

async function loadToday(): Promise<void> {
  payload = await invoke<TodayPayload | null>("get_today");
  renderToday();
}

function isPayloadMissing(): boolean {
  return payload === null;
}

// ---- Month view ----

async function loadMonth(): Promise<void> {
  $("month-label").textContent = MONTH_NAMES[monthIndex - 1];
  const table = $("month-table") as HTMLTableElement;
  table.innerHTML = "";

  const today = new Date();
  const isCurrentMonth = monthIndex === today.getMonth() + 1;
  try {
    if (monthMode === "adhan") {
      const data = await invoke<MonthTimes>("get_month", { month: monthIndex });
      renderMonthTable(table, ["Fajr", "Shurouq", "Dhuhr", "Asr", "Maghrib", "Isha"],
        data.days.map((d) => [String(d.day), d.times.fajr, d.times.shurouq, d.times.dhuhr, d.times.asr, d.times.maghrib, d.times.isha]),
        isCurrentMonth ? today.getDate() : null);
    } else {
      const data = await invoke<MonthIqamaTimes>("get_month_iqama", { month: monthIndex });
      renderMonthTable(table, ["Fajr", "Dhuhr", "Asr", "Maghrib", "Isha"],
        data.days.map((d) => [String(d.day), d.times.fajr, d.times.dhuhr, d.times.asr, d.times.maghrib, d.times.isha]),
        isCurrentMonth ? today.getDate() : null);
    }
  } catch (e) {
    toast(`Failed to load calendar: ${e}`);
  }
}

// ---- Mosque search (shared by onboarding + settings) ----

async function searchMosques(
  query: string,
  list: HTMLElement,
  onPick: (m: Mosque) => void,
): Promise<void> {
  if (!query.trim()) return;
  try {
    const mosques = await invoke<Mosque[]>("search_mosques", { query: query.trim() });
    renderResults(list, mosques, onPick);
  } catch (e) {
    toast(`Search failed: ${e}`);
  }
}

async function applyMosque(mosque: Mosque): Promise<void> {
  const cfg = await invoke<AppConfig>("get_config");
  cfg.mosque_slug = mosque.slug!;
  cfg.mosque_name = mosque.label ?? mosque.name ?? null;
  await invoke("update_config", { config: cfg });
  config = cfg;
}

async function pickOnboardingMosque(mosque: Mosque): Promise<void> {
  await applyMosque(mosque);
  showView("today");
  await loadToday().catch((e) => toast(`Failed to load prayer times: ${e}`));
}

// ---- Settings dialog ----

function openSettings(): void {
  if (!config) return;
  ($("set-iqama") as HTMLInputElement).checked = config.iqama_alerts;
  ($("set-autostart") as HTMLInputElement).checked = config.autostart;
  $("set-current-mosque").textContent =
    config.mosque_name ?? (config.mosque_slug || "None");
  $("set-results").innerHTML = "";
  $("set-status").textContent = "";
  selectedMosque = null;
  $("overlay").hidden = false;
  $("settings-dialog").hidden = false;
}

function closeSettings(): void {
  $("overlay").hidden = true;
  $("settings-dialog").hidden = true;
}

async function saveSettings(): Promise<void> {
  const status = $("set-status");
  try {
    let cfg = await invoke<AppConfig>("get_config");
    if (selectedMosque) {
      cfg.mosque_slug = selectedMosque.slug!;
      cfg.mosque_name = selectedMosque.label ?? selectedMosque.name ?? null;
    }
    cfg.iqama_alerts = ($("set-iqama") as HTMLInputElement).checked;
    cfg.autostart = ($("set-autostart") as HTMLInputElement).checked;

    await invoke("update_config", { config: cfg });
    config = cfg;

    // The autostart plugin rewrites its entry with the running binary's path
    // on every call (even a doomed AppImage mount path) — invoke it only when
    // the setting actually changed, and never from a dev session: a dev
    // binary in the entry means a broken login.
    if (!import.meta.env.DEV) {
      try {
        if (cfg.autostart !== (await isEnabled())) {
          if (cfg.autostart) await enable();
          else await disable();
        }
      } catch (e) {
        console.warn("Autostart plugin error:", e);
      }
    }

    closeSettings();
    if (isConfigured()) {
      await loadToday().catch((e) => toast(`Failed to load prayer times: ${e}`));
    } else {
      showView("onboarding");
    }
  } catch (e) {
    status.textContent = `Save failed: ${e}`;
  }
}

// ---- Prayer notifications panel ----

/// Write-through persistence: every panel interaction is applied to the
/// config immediately (like the mobile app), so there is no Save button to
/// forget and no Cancel state to keep in sync.
async function persistAlerts(): Promise<void> {
  if (!config) return;
  try {
    const cfg = await invoke<AppConfig>("get_config");
    cfg.alerts = alertsDraft;
    await invoke("update_config", { config: cfg });
    config = cfg;
  } catch (e) {
    toast(`Failed to save notification settings: ${e}`);
  }
}

function prayerAlertsLabel(key: PrayerKey): string {
  return PRAYERS.find((p) => p.key === key)?.label ?? key;
}

function buildNotifyTabs(): void {
  const tabs = $("notify-tabs");
  tabs.innerHTML = "";
  for (const key of PRAYER_ALERT_KEYS) {
    const tab = document.createElement("button");
    tab.className = "notify-tab";
    tab.type = "button";
    tab.dataset.prayer = key;
    tab.textContent = prayerAlertsLabel(key);
    tab.addEventListener("click", () => {
      notifyTab = key;
      syncNotifyPanel();
    });
    tabs.appendChild(tab);
  }
}

/// Reflect the draft state of the current prayer into every control. Any
/// config-derived string lands via textContent (the sound path is
/// attacker-writable).
function syncNotifyPanel(): void {
  document.querySelectorAll("#notify-tabs .notify-tab").forEach((tab) => {
    tab.classList.toggle("active", (tab as HTMLElement).dataset.prayer === notifyTab);
  });

  const alerts = alertsDraft[notifyTab];
  document.querySelectorAll("#mode-cards .mode-card").forEach((card) => {
    card.classList.toggle("active", (card as HTMLElement).dataset.mode === alerts.mode);
  });

  $("adhan-options").hidden = alerts.mode !== "adhan";
  const hasCustom = !!alerts.sound;
  ($("notify-sound-select") as HTMLSelectElement).value = hasCustom ? "custom" : "";
  const path = $("notify-sound-path");
  path.hidden = !hasCustom;
  path.textContent = alerts.sound ?? "";

  const volumeOn = alerts.volume !== null;
  ($("notify-volume-toggle") as HTMLInputElement).checked = volumeOn;
  $("notify-volume-row").hidden = !volumeOn;
  const volume = clampVolume(alerts.volume ?? 100);
  ($("notify-volume") as HTMLInputElement).value = String(volume);
  $("notify-volume-value").textContent = `${volume}%`;

  $("notify-before-value").textContent = notifyBeforeLabel(alerts.notify_before_min);

  $("shuruq-value").textContent = notifyBeforeLabel(alertsDraft.shuruq_notify_before_min);
}

function openNotify(): void {
  if (!config) return;
  alertsDraft = normalizeAlerts(config.alerts, config.sound_enabled);
  notifyTab = "fajr";
  syncNotifyPanel();
  $("overlay").hidden = false;
  $("notify-dialog").hidden = false;
}

function closeNotify(): void {
  $("overlay").hidden = true;
  $("notify-dialog").hidden = true;
}

async function pickSoundFile(): Promise<string | null> {
  try {
    const picked = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "Audio", extensions: ["mp3", "wav"] }],
    });
    return typeof picked === "string" ? picked : null;
  } catch (e) {
    toast(`File picker failed: ${e}`);
    return null;
  }
}

async function setMode(mode: AthanMode): Promise<void> {
  alertsDraft[notifyTab].mode = mode;
  await persistAlerts();
  syncNotifyPanel();
}

// ---- Athan stop ----

/// Show/hide the stop button from the backend's actual playback state.
async function refreshAthanButton(): Promise<void> {
  try {
    $("stop-athan-btn").hidden = !(await invoke<boolean>("athan_playing"));
  } catch {
    $("stop-athan-btn").hidden = true;
  }
}

// ---- Boot & event wiring ----

// ---- Quick-glance overlay (?glance) ----

async function bootGlance(): Promise<void> {
  document.body.classList.add("glance-mode");

  const el = (cls: string, text?: string): HTMLDivElement => {
    const node = document.createElement("div");
    node.className = cls;
    if (text !== undefined) node.textContent = text;
    return node;
  };
  const mosque = el("glance-mosque", "Mawaqit");
  const label = el("glance-label", "Next prayer");
  const name = el("glance-name", "—");
  const countdown = el("glance-countdown", "--:--");
  const offline = el("glance-offline");
  offline.hidden = true;
  const rows = el("glance-rows");
  document.body.replaceChildren(root(mosque, label, name, countdown, offline, rows));

  const render = async (): Promise<void> => {
    try {
      payload = await invoke<TodayPayload | null>("get_today");
    } catch {
      payload = null;
    }
    if (!payload) {
      mosque.textContent = "No data";
      rows.replaceChildren();
      return;
    }
    mosque.textContent = payload.mosque_name ?? config?.mosque_name ?? "Mawaqit";
    offline.hidden = !payload.as_of;
    if (payload.as_of) offline.textContent = `Offline · from ${payload.as_of}`;

    const fajrLabel = payload.imsak_mode ? "Imsak" : "Fajr";
    const lines: [string, string, string][] = [];
    for (const { key, label: prayerLabel } of PRAYERS) {
      const shown = key === "fajr" ? fajrLabel : prayerLabel;
      const iq = payload.times.iqama
        ? `iqama ${payload.times.iqama[key as keyof DailyIqamaTimes]}`
        : "";
      lines.push([shown, payload.times.adhan[key], iq]);
      if (key === "fajr") {
        lines.push(["Shurouq", payload.times.adhan.shurouq, ""]);
      }
    }
    rows.replaceChildren(
      ...lines.map(([prayerName, time, iq]) => {
        const row = el("glance-row");
        const left = el("glance-row-name", prayerName);
        const right = el("glance-row-time", time);
        if (iq) right.textContent = `${time}  ·  ${iq}`;
        row.append(left, right);
        return row;
      }),
    );
  };

  const tickGlance = (): void => {
    if (!payload) return;
    const next = computeNextEvent();
    if (!next) return;
    name.textContent = next.label;
    countdown.textContent = formatCountdown(next.at.getTime() - Date.now());
  };

  await render();
  tickGlance();
  window.setInterval(tickGlance, 1000);
  window.setInterval(() => void render(), 60_000);
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape") void getCurrentWindow().hide();
  });
}

function root(...children: HTMLElement[]): HTMLDivElement {
  const root = document.createElement("div");
  root.id = "glance";
  root.append(...children);
  return root;
}

async function boot(): Promise<void> {
  try {
    await loadConfig();
  } catch (e) {
    console.error("Failed to load config:", e);
    config = { ...DEFAULT_CONFIG };
  }

  if (isConfigured()) {
    showView("today");
    await loadToday().catch((e) => toast(`Failed to load prayer times: ${e}`));
    if (isPayloadMissing()) {
      toast("Could not load prayer times — check your connection or pick another mosque.");
    }
  } else {
    showView("onboarding");
  }
}

document.addEventListener("DOMContentLoaded", () => {
  // The quick-glance overlay (?glance) renders its own compact view and
  // shares nothing with the main app's DOM wiring below.
  if (new URLSearchParams(location.search).has("glance")) {
    void bootGlance();
    return;
  }

  boot();

  $("tab-today").addEventListener("click", () => showView("today"));
  $("tab-month").addEventListener("click", () => {
    showView("month");
    loadMonth();
  });
  $("month-prev").addEventListener("click", () => {
    monthIndex = monthIndex === 1 ? 12 : monthIndex - 1;
    loadMonth();
  });
  $("month-next").addEventListener("click", () => {
    monthIndex = monthIndex === 12 ? 1 : monthIndex + 1;
    loadMonth();
  });
  $("seg-adhan").addEventListener("click", () => {
    monthMode = "adhan";
    $("seg-adhan").classList.add("active");
    $("seg-iqama").classList.remove("active");
    loadMonth();
  });
  $("seg-iqama").addEventListener("click", () => {
    monthMode = "iqama";
    $("seg-iqama").classList.add("active");
    $("seg-adhan").classList.remove("active");
    loadMonth();
  });

  $("settings-btn").addEventListener("click", openSettings);
  $("cancel-btn").addEventListener("click", closeSettings);
  $("save-btn").addEventListener("click", saveSettings);
  $("overlay").addEventListener("click", () => {
    closeSettings();
    closeNotify();
  });

  // ---- Prayer notifications panel ----
  buildNotifyTabs();
  $("notify-btn").addEventListener("click", openNotify);
  $("notify-close").addEventListener("click", closeNotify);
  document.querySelectorAll<HTMLElement>("#mode-cards .mode-card").forEach((card) => {
    card.addEventListener("click", () =>
      setMode((card.dataset.mode ?? "adhan") as AthanMode),
    );
  });

  $("notify-sound-select").addEventListener("change", async () => {
    const select = $("notify-sound-select") as HTMLSelectElement;
    if (select.value === "custom") {
      const picked = await pickSoundFile();
      if (picked) {
        alertsDraft[notifyTab].sound = picked;
        await persistAlerts();
      } else {
        // Picker cancelled: fall back to whatever the draft still holds.
        select.value = alertsDraft[notifyTab].sound ? "custom" : "";
        return;
      }
    } else {
      alertsDraft[notifyTab].sound = null;
      await persistAlerts();
    }
    syncNotifyPanel();
  });

  $("notify-preview").addEventListener("click", async () => {
    const alerts = alertsDraft[notifyTab];
    try {
      await invoke("preview_athan", { sound: alerts.sound, volume: alerts.volume });
      await refreshAthanButton();
    } catch (e) {
      toast(`Preview failed: ${e}`);
    }
  });

  $("notify-volume-toggle").addEventListener("change", async () => {
    const on = ($("notify-volume-toggle") as HTMLInputElement).checked;
    if (on) {
      alertsDraft[notifyTab].volume = clampVolume(
        Number(($("notify-volume") as HTMLInputElement).value) || 100,
      );
    } else {
      alertsDraft[notifyTab].volume = null;
    }
    await persistAlerts();
    syncNotifyPanel();
  });
  $("notify-volume").addEventListener("input", () => {
    const v = clampVolume(Number(($("notify-volume") as HTMLInputElement).value));
    $("notify-volume-value").textContent = `${v}%`;
  });
  $("notify-volume").addEventListener("change", async () => {
    alertsDraft[notifyTab].volume = clampVolume(
      Number(($("notify-volume") as HTMLInputElement).value),
    );
    await persistAlerts();
  });

  $("notify-before-minus").addEventListener("click", async () => {
    alertsDraft[notifyTab].notify_before_min = stepNotifyBefore(
      alertsDraft[notifyTab].notify_before_min,
      -1,
    );
    await persistAlerts();
    syncNotifyPanel();
  });
  $("notify-before-plus").addEventListener("click", async () => {
    alertsDraft[notifyTab].notify_before_min = stepNotifyBefore(
      alertsDraft[notifyTab].notify_before_min,
      1,
    );
    await persistAlerts();
    syncNotifyPanel();
  });

  $("shuruq-minus").addEventListener("click", async () => {
    alertsDraft.shuruq_notify_before_min = stepNotifyBefore(
      alertsDraft.shuruq_notify_before_min,
      -1,
    );
    await persistAlerts();
    syncNotifyPanel();
  });
  $("shuruq-plus").addEventListener("click", async () => {
    alertsDraft.shuruq_notify_before_min = stepNotifyBefore(
      alertsDraft.shuruq_notify_before_min,
      1,
    );
    await persistAlerts();
    syncNotifyPanel();
  });

  $("notify-apply-all").addEventListener("click", async () => {
    alertsDraft = applyToAll(alertsDraft, notifyTab);
    await persistAlerts();
    syncNotifyPanel();
  });

  $("ob-search-btn").addEventListener("click", () =>
    searchMosques(($("ob-search") as HTMLInputElement).value, $("ob-results"), pickOnboardingMosque),
  );
  $("ob-search").addEventListener("keydown", (e) => {
    if ((e as KeyboardEvent).key === "Enter") {
      e.preventDefault();
      searchMosques(($("ob-search") as HTMLInputElement).value, $("ob-results"), pickOnboardingMosque);
    }
  });

  $("set-search-btn").addEventListener("click", () =>
    searchMosques(
      ($("set-search") as HTMLInputElement).value,
      $("set-results"),
      (m) => {
        selectedMosque = m;
        $("set-current-mosque").textContent = mosqueDisplayName(m);
      },
    ),
  );

  window.setInterval(tick, 1000);

  $("stop-athan-btn").addEventListener("click", async () => {
    try {
      await invoke("stop_athan");
    } catch (e) {
      toast(`Failed to stop athan: ${e}`);
    }
    $("stop-athan-btn").hidden = true;
  });
  // Emitted by the backend right when the athan starts sounding.
  listen("athan-started", () => refreshAthanButton()).catch(console.warn);
  refreshAthanButton();
});
