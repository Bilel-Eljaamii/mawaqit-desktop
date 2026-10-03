// Pure display helpers extracted from main.ts so the red-team suite can
// hammer them without a Tauri runtime. Every string these functions receive
// is attacker-influenced: it originates from mawaqit.net (mosque names,
// times, jumua strings, image URLs) and reaches this code over IPC.

// ---- Types shared by the app and its tests ----

export interface Mosque {
  uuid?: string | null;
  slug?: string | null;
  name?: string | null;
  label?: string | null;
  locality?: string | null;
  country?: string | null;
}

export interface DailyPrayerTimes {
  fajr: string;
  shurouq: string;
  dhuhr: string;
  asr: string;
  maghrib: string;
  isha: string;
}

export interface DailyIqamaTimes {
  fajr: string;
  dhuhr: string;
  asr: string;
  maghrib: string;
  isha: string;
}

/** Best human-readable name: label, then name, then the slug. */
export function mosqueDisplayName(m: Mosque): string {
  return m.label ?? m.name ?? m.slug ?? "?";
}

/** Only allow plain http(s) URLs into CSS url(...); escape anything a parser
 * could use to escape the string (quotes, backslashes, parens, whitespace).
 * Emits the *parsed* URL (F11): the URL parser strips tab/newline control
 * characters before the scheme check, so escaping the raw string would send
 * CSS a different URL than the one that was validated. */
export function sanitizeCssUrl(raw: string): string | null {
  let url: URL;
  try {
    url = new URL(raw);
    if (url.protocol !== "https:" && url.protocol !== "http:") return null;
  } catch {
    return null;
  }
  const normalized = url.toString();
  const escapes: Record<string, string> = {
    '"': "%22", "'": "%27", "\\": "%5C", "(": "%28", ")": "%29",
    " ": "%20", "\n": "%0a", "\r": "%0d", "\t": "%09",
  };
  return normalized.replace(/["'\\()\s]/g, (c) => escapes[c] ?? encodeURIComponent(c));
}

/** Parse an "HH:MM" string (from the wire) into today's Date at that time,
 * or null when it is not a plausible time. Strict: hours 00–23, minutes
 * 00–59 — "25:70" must be rejected, never rolled over by setHours. */
export function parseHhmmToDate(hhmm: string, dayOffset = 0): Date | null {
  const m = /^(\d{2}):(\d{2})$/.exec(hhmm.trim());
  if (!m) return null;
  const hours = parseInt(m[1], 10);
  const minutes = parseInt(m[2], 10);
  if (hours > 23 || minutes > 59) return null;
  const d = new Date();
  d.setDate(d.getDate() + dayOffset);
  d.setHours(hours, minutes, 0, 0);
  return d;
}

export function formatCountdown(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
}

/** Month table: hostile cells must land as inert text (textContent), never
 * as markup. `highlightDay` marks the current day's row when the displayed
 * month is the current one. */
export function renderMonthTable(
  table: HTMLTableElement,
  headers: string[],
  rows: string[][],
  highlightDay: number | null = null,
): void {
  const thead = document.createElement("thead");
  const headRow = document.createElement("tr");
  for (const h of ["Day", ...headers]) {
    const th = document.createElement("th");
    th.textContent = h;
    headRow.appendChild(th);
  }
  thead.appendChild(headRow);
  table.appendChild(thead);

  const tbody = document.createElement("tbody");
  for (const row of rows) {
    const tr = document.createElement("tr");
    if (highlightDay !== null && row[0] === String(highlightDay)) {
      tr.className = "today-row";
    }
    for (const cell of row) {
      const td = document.createElement("td");
      td.textContent = cell;
      tr.appendChild(td);
    }
    tbody.appendChild(tr);
  }
  table.appendChild(tbody);
}

/** Search results: hostile mosque names must land as inert text. */
export function renderResults(
  list: HTMLElement,
  mosques: Mosque[],
  onPick: (m: Mosque) => void,
): void {
  list.innerHTML = "";
  const usable = mosques.filter((m) => m.slug);
  if (usable.length === 0) {
    const li = document.createElement("li");
    li.className = "empty";
    li.textContent = "No mosque found — try another keyword.";
    list.appendChild(li);
    return;
  }
  for (const mosque of usable) {
    const li = document.createElement("li");
    const place = [mosque.locality, mosque.country].filter(Boolean).join(", ");
    li.textContent = place ? `${mosqueDisplayName(mosque)} — ${place}` : mosqueDisplayName(mosque);
    li.addEventListener("click", () => onPick(mosque));
    list.appendChild(li);
  }
}
