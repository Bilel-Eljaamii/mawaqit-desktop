import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { enable, disable } from "@tauri-apps/plugin-autostart";
import {
  formatCountdown,
  mosqueDisplayName,
  parseHhmmToDate,
  renderMonthTable,
  renderResults,
  sanitizeCssUrl,
} from "./lib/display";
import type { DailyIqamaTimes, DailyPrayerTimes, Mosque } from "./lib/display";

// ---- Types mirroring the Rust serde structs ----

interface AppConfig {
  mosque_slug: string;
  mosque_name: string | null;
  sound_enabled: boolean;
  iqama_alerts: boolean;
  autostart: boolean;
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
};

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
  ($("set-sound") as HTMLInputElement).checked = config.sound_enabled;
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
    cfg.sound_enabled = ($("set-sound") as HTMLInputElement).checked;
    cfg.iqama_alerts = ($("set-iqama") as HTMLInputElement).checked;
    cfg.autostart = ($("set-autostart") as HTMLInputElement).checked;

    await invoke("update_config", { config: cfg });
    config = cfg;

    try {
      if (cfg.autostart) await enable();
      else await disable();
    } catch (e) {
      console.warn("Autostart plugin error:", e);
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
  $("overlay").addEventListener("click", closeSettings);

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
