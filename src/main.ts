import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getVersion } from "@tauri-apps/api/app";
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
import {
  markAllRead as markAllReadHelper,
  markRead as markReadHelper,
  unreadCount as countUnread,
  visibleAnnouncements,
} from "./lib/announcements";
import type { AnnouncementItem } from "./lib/announcements";

// ---- Types mirroring the Rust serde structs ----

interface AppConfig {
  mosque_slug: string;
  mosque_name: string | null;
  sound_enabled: boolean;
  iqama_alerts: boolean;
  autostart: boolean;
  /** Serve saved (snapshot) times only; never touch the network for data. */
  offline_mode: boolean;
  /** Tor transport policy: route mawaqit.net through socks5h://host:port. */
  tor: { enabled: boolean; host: string; port: number };
  alerts: AlertsConfig;
  /** Ids of mosque announcements the user has read (capped backend-side). */
  announcements_read: string[];
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
  /** The mosque's announcements (wire order), ids normalized. */
  announcements: AnnouncementItem[];
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
  offline_mode: false,
  tor: { enabled: false, host: "127.0.0.1", port: 9050 },
  alerts: defaultAlerts(),
  announcements_read: [],
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
  refreshAnnounceBadge();
  syncTorButton();
}

function syncTorButton(): void {
  const btn = $("tor-btn");
  if (!btn) return;
  const on = config?.tor.enabled ?? false;
  btn.classList.toggle("active", on);
  btn.setAttribute("aria-pressed", String(on));
  btn.title = on
    ? `Tor on — socks5h://${config?.tor.host}:${config?.tor.port} (click to turn off)`
    : "Tor off (click to route mawaqit.net through Tor)";
}

// ---- Online / offline toggle ----

function applyOfflineUi(): void {
  const on = config?.offline_mode ?? false;
  const btn = $("offline-btn");
  btn.classList.toggle("active", on);
  btn.setAttribute("aria-pressed", String(on));
  btn.title = on
    ? "Offline mode on — showing saved times only (click to go back online)"
    : "Go offline — use only the times saved on this device";
  $("offline-icon-on").hidden = !on;
  $("offline-icon-off").hidden = on;
}

async function toggleOffline(): Promise<void> {
  if (!config) return;
  const on = !config.offline_mode;
  try {
    await invoke("set_offline_mode", { on });
    config.offline_mode = on;
    applyOfflineUi();
    toast(
      on ? "Offline mode on — showing saved times" : "Back online — times will refresh",
      false,
    );
    await loadToday().catch((e) => toast(`Failed to load prayer times: ${e}`));
    if (!$("view-month").hidden) {
      loadMonth();
    }
  } catch (e) {
    toast(`Could not switch offline mode: ${e}`);
  }
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
  const tor = ($("set-tor") as HTMLInputElement);
  const torHost = $("set-tor-host") as HTMLInputElement;
  const torPort = $("set-tor-port") as HTMLInputElement;
  tor.checked = config.tor.enabled;
  torHost.value = config.tor.host;
  torPort.value = String(config.tor.port);
  torHost.disabled = !tor.checked;
  torPort.disabled = !tor.checked;
  ($("set-iqama") as HTMLInputElement).checked = config.iqama_alerts;
  ($("set-autostart") as HTMLInputElement).checked = config.autostart;
  $("set-status").textContent = "";
  $("overlay").hidden = false;
  $("settings-dialog").hidden = false;
}

// ---- Mosque dialog (switching) ----

function openMosque(): void {
  if (!config) return;
  $("set-current-mosque").textContent =
    config.mosque_name ?? (config.mosque_slug || "None");
  $("set-results").innerHTML = "";
  $("set-status").textContent = "";
  $("overlay").hidden = false;
  $("mosque-dialog").hidden = false;
}

function closeMosque(): void {
  $("mosque-dialog").hidden = true;
  if ($("announce-dialog").hidden && $("settings-dialog").hidden) {
    $("overlay").hidden = true;
  }
}

async function pickMosqueFromDialog(mosque: Mosque): Promise<void> {
  await applyMosque(mosque);
  closeMosque();
  showView("today");
  await loadToday().catch((e) => toast(`Failed to load prayer times: ${e}`));
  refreshAnnounceBadge();
}

// ---- Mosque announcements inbox ----

function currentAnnouncements(): AnnouncementItem[] {
  return visibleAnnouncements(payload?.announcements ?? []);
}

function refreshAnnounceBadge(): void {
  const unread = countUnread(
    payload?.announcements ?? [],
    config?.announcements_read ?? [],
  );
  const badge = $("announce-badge");
  badge.hidden = unread === 0;
  badge.textContent = unread > 99 ? "99+" : unread === 0 ? "" : String(unread);
}

async function persistAnnouncementsRead(read: string[]): Promise<void> {
  if (!config) return;
  try {
    const cfg = await invoke<AppConfig>("get_config");
    cfg.announcements_read = read;
    await invoke("update_config", { config: cfg });
    config = cfg;
  } catch (e) {
    toast(`Failed to save read state: ${e}`);
  }
}

function renderAnnounceList(): void {
  const items = currentAnnouncements();
  const read = new Set(config?.announcements_read ?? []);
  const list = $("announce-list");
  list.replaceChildren();

  if (items.length === 0) {
    const empty = document.createElement("li");
    empty.className = "announce-empty";
    empty.textContent = "No announcements from your mosque.";
    list.appendChild(empty);
  }
  for (const a of items) {
    const isRead = read.has(a.id);
    const row = document.createElement("li");
    row.className = isRead ? "announce-row read" : "announce-row";
    row.dataset.id = a.id;

    const dot = document.createElement("span");
    dot.className = "announce-dot";
    if (!isRead) dot.textContent = "●";
    const body = document.createElement("div");
    body.className = "announce-body";
    const head = document.createElement("div");
    head.className = "announce-title";
    head.textContent = a.title ?? a.content ?? "(announcement)";
    body.appendChild(head);
    if (a.title !== null && a.content !== null && a.content.trim() !== "") {
      body.appendChild(el2("announce-content", a.content));
    }
    const meta = [a.start_date, a.end_date].filter(Boolean).join(" → ");
    if (meta) {
      body.appendChild(el2("announce-date", meta));
    }
    row.append(dot, body);
    row.addEventListener("click", async () => {
      if (isRead) return;
      const next = markReadHelper(config?.announcements_read ?? [], a.id);
      await persistAnnouncementsRead(next);
      renderAnnounceList();
      refreshAnnounceBadge();
    });
    list.appendChild(row);
  }

  // Counts and mark-all cover the FULL announcement list — the 50-item cap
  // is render-only, so the toolbar and the badge can never disagree.
  const unread = countUnread(payload?.announcements ?? [], config?.announcements_read ?? []);
  $("announce-unread").textContent =
    unread === 0 ? "All caught up." : `${unread} unread`;
  ($("announce-mark-all") as HTMLButtonElement).hidden = unread === 0;
}

function voiceRow(
  name: string,
  selected: boolean,
  onSelect: () => void,
): HTMLLIElement {
  const row = document.createElement("li");
  row.className = selected ? "voice-entry selected" : "voice-entry";
  const state = document.createElement("span");
  state.className = "voice-state";
  if (selected) state.textContent = "\u2713";
  const label = document.createElement("span");
  label.className = "voice-name";
  label.textContent = name;
  row.append(state, label);
  row.addEventListener("click", () => void onSelect());
  return row;
}

function el2(cls: string, text?: string): HTMLDivElement {
  const node = document.createElement("div");
  node.className = cls;
  if (text !== undefined) node.textContent = text;
  return node;
}

function openAnnounce(): void {
  renderAnnounceList();
  $("overlay").hidden = false;
  $("announce-dialog").hidden = false;
}

function closeAnnounce(): void {
  $("announce-dialog").hidden = true;
  if ($("mosque-dialog").hidden && $("settings-dialog").hidden) {
    $("overlay").hidden = true;
  }
}

async function markAllAnnouncementsRead(): Promise<void> {
  const next = markAllReadHelper(currentAnnouncements(), config?.announcements_read ?? []);
  await persistAnnouncementsRead(next);
  renderAnnounceList();
  refreshAnnounceBadge();
}

function closeSettings(): void {
  $("overlay").hidden = true;
  $("settings-dialog").hidden = true;
}

async function saveSettings(): Promise<void> {
  const status = $("set-status");
  try {
    // Mosque switching applies immediately from its own dialog; this dialog
    // only carries the toggles.
    let cfg = await invoke<AppConfig>("get_config");
    // Tor opt-in: an unchecked box means direct connection; a checked one
    // requires a plausible host (backend re-validates and rejects the save
    // with a visible message otherwise).
    const tor = ($("set-tor") as HTMLInputElement);
    const torHost = ($("set-tor-host") as HTMLInputElement).value.trim();
    const torPort = Number(($("set-tor-port") as HTMLInputElement).value);
    if (tor.checked && (torHost === "" || !Number.isInteger(torPort) || torPort < 1 || torPort > 65535)) {
      toast("Enter a Tor proxy host and port (1–65535) or untick the Tor option.");
      return;
    }
    cfg.tor = { enabled: tor.checked, host: torHost || "127.0.0.1", port: torPort };
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
    toast("Settings saved", false);
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
// ---- Athan voice sheet ----

interface VoiceEntry {
  id: string;
  name: string;
  fajr_variant: boolean;
}

let voiceCatalog: VoiceEntry[] | null = null;

async function ensureVoiceCatalog(): Promise<VoiceEntry[]> {
  if (!voiceCatalog) {
    try {
      voiceCatalog = await invoke<VoiceEntry[]>("adhan_voices");
    } catch (e) {
      console.warn("voice catalog failed:", e);
      voiceCatalog = [];
    }
  }
  return voiceCatalog;
}

/// Human label for whatever the active prayer's athan source is.
function voiceSourceLabel(alerts: { voice: string | null; sound: string | null }): string {
  if (alerts.voice) {
    const entry = voiceCatalog?.find((v) => v.id === alerts.voice);
    return entry ? entry.name : alerts.voice;
  }
  if (alerts.sound) return "Custom file";
  return "Built-in athan";
}

/// Whether a voice is already downloaded (cached mp3 in the voices dir).
async function voiceIsCached(id: string): Promise<boolean> {
  try {
    return await invoke<boolean>("voice_is_cached", { id });
  } catch {
    return false;
  }
}

function toggleVoiceSheetSync(): void {
  $("voice-sheet").hidden = !$("voice-sheet").hidden;
}

async function toggleVoiceSheet(): Promise<void> {
  toggleVoiceSheetSync();
  if ($("voice-sheet").hidden) return;
  await renderVoiceList();
}

function isFajrTab(tab: PrayerKey): boolean {
  return tab === "fajr";
}

async function renderVoiceList(): Promise<void> {
  const list = $("voice-list");
  list.replaceChildren();

  const voices = await ensureVoiceCatalog();
  const alerts = alertsDraft[notifyTab];
  const fajrTab = isFajrTab(notifyTab);
  const entries = voices.filter((v) => fajrTab || !v.fajr_variant);

  // Built-in athan is always first and always available.
  list.appendChild(voiceRow("Built-in athan", alerts.voice === null, async () => {
    alertsDraft[notifyTab].voice = null;
    await persistAlerts();
    syncNotifyPanel();
  }));

  for (const v of entries) {
    const selected = alerts.voice === v.id;
    const row = document.createElement("li");
    row.className = "voice-entry";
    row.dataset.id = v.id;

    const play = document.createElement("button");
    play.className = "voice-play";
    play.type = "button";
    play.title = "Preview";
    play.textContent = "\u25B6";
    play.addEventListener("click", async (e) => {
      e.stopPropagation();
      try {
        await invoke("preview_athan", {
          sound: null,
          voiceId: v.id,
          volume: alerts.volume,
        });
        await refreshAthanButton();
      } catch (err) {
        toast(`Preview failed: ${err}`);
      }
    });
    row.appendChild(play);

    const name = document.createElement("span");
    name.className = "voice-name";
    name.textContent = v.name;
    row.appendChild(name);

    const state = document.createElement("span");
    state.className = "voice-state";
    if (selected) {
      state.textContent = "\u2713";
      row.classList.add("selected");
    } else {
      void voiceIsCached(v.id).then((cached) => {
        if (!cached) state.textContent = "\u2934"; // needs download
      });
    }
    row.appendChild(state);

    row.addEventListener("click", async () => {
      alertsDraft[notifyTab].voice = v.id;
      // A catalog voice supersedes the custom file for this prayer.
      alertsDraft[notifyTab].sound = null;
      // The selection lands first — the download must never gate the UI.
      // Offline or CDN-down, awaiting download_voice before re-rendering
      // hung the sheet for the whole transport timeout. On failure the
      // selection stays: builtin plays now, the next adhan retries.
      await persistAlerts();
      syncNotifyPanel();
      await renderVoiceList();
      try {
        await invoke("download_voice", { voiceId: v.id });
      } catch (e) {
        toast(`Voice not downloaded yet — builtin athan will play: ${e}`);
      }
    });
    list.appendChild(row);
  }

  // Custom file entry last, like the old select's "custom" option.
  const customRow = document.createElement("li");
  customRow.className = "voice-entry voice-custom";
  customRow.textContent = "Custom file\u2026";
  customRow.addEventListener("click", async () => {
    const picked = await pickSoundFile();
    if (picked) {
      alertsDraft[notifyTab].voice = null;
      alertsDraft[notifyTab].sound = picked;
      await persistAlerts();
      syncNotifyPanel();
      toggleVoiceSheetSync();
    }
  });
  list.appendChild(customRow);
}

function syncNotifyPanel(): void {
  document.querySelectorAll("#notify-tabs .notify-tab").forEach((tab) => {
    tab.classList.toggle("active", (tab as HTMLElement).dataset.prayer === notifyTab);
  });

  const alerts = alertsDraft[notifyTab];
  document.querySelectorAll("#mode-cards .mode-card").forEach((card) => {
    card.classList.toggle("active", (card as HTMLElement).dataset.mode === alerts.mode);
  });

  $("adhan-options").hidden = alerts.mode !== "adhan";
  $("voice-current").textContent = voiceSourceLabel(alerts);

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
  // Mouse-closable at all times: the blur-hide is best-effort (focus events
  // are not guaranteed for an always-on-top overlay), so the overlay must
  // never depend on the keyboard to go away.
  const closeBtn = document.createElement("button");
  closeBtn.className = "glance-close";
  closeBtn.title = "Close";
  closeBtn.textContent = "\u2715";
  closeBtn.addEventListener("click", () => void getCurrentWindow().hide());
  const mosque = el("glance-mosque", "Mawaqit");
  const label = el("glance-label", "Next prayer");
  const name = el("glance-name", "—");
  const countdown = el("glance-countdown", "--:--");
  const offline = el("glance-offline");
  offline.hidden = true;
  const rows = el("glance-rows");
  document.body.replaceChildren(root(closeBtn, mosque, label, name, countdown, offline, rows));

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

  applyOfflineUi();

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

  // Settings dialog footer: app version (comes from tauri.conf.json).
  void getVersion()
    .then((v) => {
      $("app-version").textContent = `Mawaqit Desktop v${v}`;
    })
    .catch((e) => console.warn("getVersion failed:", e));

  $("settings-btn").addEventListener("click", openSettings);
  // Topbar Tor toggle: flips immediately (backend swaps the transport).
  $("tor-btn").addEventListener("click", async () => {
    if (!config) return;
    try {
      const on = await invoke<boolean>("set_tor_enabled", { on: !config.tor.enabled });
      config.tor.enabled = on;
      const btn = $("tor-btn");
      btn.classList.toggle("active", on);
      btn.setAttribute("aria-pressed", String(on));
      toast(on ? "Tor on" : "Tor off", false);
    } catch (e) {
      toast(`Tor toggle failed: ${e}`);
    }
  });
  const syncTorFields = (): void => {
    const on = ($("set-tor") as HTMLInputElement).checked;
    ($("set-tor-host") as HTMLInputElement).disabled = !on;
    ($("set-tor-port") as HTMLInputElement).disabled = !on;
  };
  $("set-tor").addEventListener("change", syncTorFields);
  $("offline-btn").addEventListener("click", () => void toggleOffline());
  $("mosque-btn").addEventListener("click", openMosque);
  $("mosque-cancel-btn").addEventListener("click", closeMosque);
  $("announce-btn").addEventListener("click", openAnnounce);
  $("announce-close").addEventListener("click", closeAnnounce);
  $("announce-mark-all").addEventListener("click", () => void markAllAnnouncementsRead());
  $("cancel-btn").addEventListener("click", closeSettings);
  $("save-btn").addEventListener("click", saveSettings);
  $("overlay").addEventListener("click", () => {
    closeSettings();
    closeNotify();
    closeMosque();
    closeAnnounce();
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

  // Athan voice sheet.
  $("voice-row").addEventListener("click", () => void toggleVoiceSheet());
  $("voice-sheet-close").addEventListener("click", () => toggleVoiceSheetSync());
  $("notify-preview").addEventListener("click", async () => {
    const alerts = alertsDraft[notifyTab];
    const voiceId = alerts.voice ?? null;
    const sound = voiceId ? null : alerts.sound;
    try {
      await invoke("preview_athan", { sound, voiceId, volume: alerts.volume });
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

  const runMosqueSearch = (): void => {
    searchMosques(
      ($("set-search") as HTMLInputElement).value,
      $("set-results"),
      (m) => {
        $("set-current-mosque").textContent = mosqueDisplayName(m);
        void pickMosqueFromDialog(m);
      },
    );
  };
  $("set-search-btn").addEventListener("click", runMosqueSearch);
  $("set-search").addEventListener("keydown", (e) => {
    if ((e as KeyboardEvent).key === "Enter") {
      e.preventDefault();
      runMosqueSearch();
    }
  });
  // Live suggestions: after 4 letters, search as you type (debounced).
  let suggestTimer: number | null = null;
  $("set-search").addEventListener("input", () => {
    const value = ($("set-search") as HTMLInputElement).value.trim();
    if (suggestTimer !== null) window.clearTimeout(suggestTimer);
    if (value.length < 4) return;
    suggestTimer = window.setTimeout(runMosqueSearch, 300);
  });

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
