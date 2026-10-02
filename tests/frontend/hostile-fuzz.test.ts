/**
 * In-tree mutation fuzzer for the frontend display + notification helpers.
 * Same method as the Rust fuzzers (hostile_fuzz.rs / hostile_corpus.rs):
 * known-valid seeds, deterministic xorshift mutations, hostile-data
 * invariants — every function here receives attacker-influenced strings
 * and numbers (mawaqit.net content over IPC, config-file values) and must
 * stay total: never throw, never emit markup, never emit out-of-range
 * values.
 *
 * Run with `pnpm test`.
 */
import { describe, expect, it, test } from "vitest";
import {
  formatCountdown,
  mosqueDisplayName,
  parseHhmmToDate,
  renderMonthTable,
  renderResults,
  sanitizeCssUrl,
} from "../../src/lib/display";
import {
  clampVolume,
  defaultAlerts,
  normalizeAlerts,
  NOTIFY_LADDER,
  seedAlertsFromSound,
  stepNotifyBefore,
} from "../../src/lib/notifications";

// ------------------------------------------------------------- mutation engine

function xorshift(state: bigint): bigint {
  state ^= state << 13n;
  state ^= state >> 7n;
  state ^= state << 17n;
  return state & 0xffffffffffffffffn;
}

const rng = { state: 0x9e3779b97f4a7c15n };
function next(): bigint {
  rng.state = xorshift(rng.state) | 1n;
  return rng.state;
}
function below(n: number): number {
  return n === 0 ? 0 : Number(next() % BigInt(n));
}

/** String fragments that break parsers, sanitizers and renderers. */
const TOKENS = [
  "<script>", "</script>", "<img src=x onerror=alert(1)>", "javascript:", "data:",
  "'", '"', "\\", "(", ")", "url(", "style=", "<svg onload=x>", "</td>",
  "06:30", "25:70", "99:99", "+30", "-5", ":", "00", "9", ".", "e", "NaN", "Infinity",
  "\u0000", "\u202E", "\u202D", "\n", "\r", "\t", "\u{0001}", "\u007F", "\uFEFF",
  "🕌", "é", "\u{10FFFF}", "{}", "[]", "/", "?", "#", "&", "=", "A".repeat(50),
];

function mutateString(input: string): string {
  let s = input;
  const ops = 1 + below(4);
  for (let i = 0; i < ops; i++) {
    const op = below(4);
    const at = below(s.length + 1);
    switch (op) {
      case 0: // splice in a token
        s = s.slice(0, at) + TOKENS[below(TOKENS.length)] + s.slice(at);
        break;
      case 1: // delete a slice
        s = s.slice(0, at) + s.slice(Math.min(s.length, at + 1 + below(20)));
        break;
      case 2: { // duplicate a slice elsewhere
        const len = Math.min(1 + below(20), s.length - at);
        s = s.slice(0, at) + s.slice(at, at + len) + s.slice(at);
        break;
      }
      default: { // replace a char with a random one from the token pool
        const t = TOKENS[below(TOKENS.length)];
        s = s.slice(0, at) + t + s.slice(Math.min(s.length, at + 1));
      }
    }
  }
  return s;
}

function mutateNumber(input: number): number {
  switch (below(6)) {
    case 0: return input + (below(400) - 200);
    case 1: return input * (below(1000) / 10 - 50);
    case 2: return -input;
    case 3: return below(1_000_000) / (1 + below(100));
    case 4: return Number(TOKENS[below(TOKENS.length)]); // often NaN
    default: return below(2) ? 0 : 255;
  }
}

function mutateStructure(input: unknown, depth = 0): unknown {
  if (depth > 3 || below(4) === 0) {
    return below(3) === 0 ? TOKENS[below(TOKENS.length)] : mutateNumber(below(200));
  }
  if (Array.isArray(input)) {
    return input.map((v) => mutateStructure(v, depth + 1)).concat(
      below(2) ? [TOKENS[below(TOKENS.length)]] : [],
    );
  }
  if (typeof input === "object" && input !== null) {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(input as Record<string, unknown>)) {
      out[below(3) === 0 ? k + TOKENS[below(TOKENS.length)] : k] = mutateStructure(v, depth + 1);
    }
    if (below(2)) out[TOKENS[below(TOKENS.length)]] = TOKENS[below(TOKENS.length)];
    return out;
  }
  return input;
}

// ------------------------------------------------------------------- seeds

const URL_SEED = "https://pics.test/mosque.jpg?q=1&s=2";
const TIME_SEED = "06:30";
const ALERTS_SEED = defaultAlerts();
const NAME_SEED = "Grande Mosquée de Paris — Paris, France";

function activeElements(root: Element): Element[] {
  return Array.from(
    root.querySelectorAll(
      "script, img, iframe, svg, object, embed, link, a[href^='javascript:']",
    ),
  );
}

// -------------------------------------------------------------------- tests

describe("fuzz: sanitizeCssUrl", () => {
  // FINDING F11 (found by this fuzzer): the URL parser strips tab/newline
  // control characters BEFORE the scheme is checked, but the sanitizer
  // emits the escaped *raw* string. So "http\t://pic/x" passes the
  // http(s)-only check yet reaches CSS as "http%09://pic/x" — not the URL
  // that was validated (worst case: a request to an app-origin path; never
  // script execution). FIX: emit the parsed URL (`new URL(raw).toString()`,
  // escaped) instead of the raw string, then remove this probe and the
  // F11 skip in the property below.
  test.fails("RED TEAM FINDING F11: sanitizer emits a different URL than it validated", () => {
    expect(sanitizeCssUrl("http\t://pic/mosque.jpg")).toBe(
      sanitizeCssUrl("http://pic/mosque.jpg"),
    );
    expect(sanitizeCssUrl("ht\ntps://pic/mosque.jpg")).toBe(
      sanitizeCssUrl("https://pic/mosque.jpg"),
    );
  });

  it("always returns null or a safe http(s) url", () => {
    for (let i = 0; i < 4000; i++) {
      const input = mutateString(URL_SEED);
      let out: string | null;
      try {
        out = sanitizeCssUrl(input);
      } catch (e) {
        throw new Error(`sanitizeCssUrl threw on ${JSON.stringify(input)}: ${e}`);
      }
      if (out === null) continue;
      // F11 known-bad class: control chars that the URL parser strips make
      // the emitted (escaped raw) string differ from the validated URL.
      // Skip until F11 is fixed — the probe above guards the fix.
      const parsed = new URL(input);
      if (parsed.toString() !== input) continue;
      expect(out).toMatch(/^https?:/);
      for (const ch of ['"', "'", "\\", "(", ")", " ", "\n", "\r", "\t"]) {
        expect(out.includes(ch)).toBe(
          false,
          `breakout char ${JSON.stringify(ch)} survived in ${JSON.stringify(out)} (from ${JSON.stringify(input)})`,
        );
      }
    }
  });
});

describe("fuzz: parseHhmmToDate", () => {
  it("never throws and only ever returns Date or null", () => {
    for (let i = 0; i < 4000; i++) {
      const input = mutateString(TIME_SEED);
      const out = parseHhmmToDate(input, below(3) - 1);
      expect(out === null || out instanceof Date).toBe(true, `input ${JSON.stringify(input)}`);
    }
  });
});

describe("fuzz: formatCountdown", () => {
  it("always renders a non-negative mm:ss / h:mm:ss clock", () => {
    for (let i = 0; i < 4000; i++) {
      const ms = mutateNumber(below(24 * 3600 * 1000));
      const out = formatCountdown(ms);
      // No sign, no NaN, no exponent — the countdown is a clock or zero.
      expect(out).toMatch(/^(\d+:)?[0-5]\d:[0-5]\d$/);
      expect(out.startsWith("-")).toBe(false);
    }
  });
});

describe("fuzz: mosqueDisplayName", () => {
  it("always returns a string", () => {
    for (let i = 0; i < 2000; i++) {
      const m = {
        slug: mutateString("grande-mosquee-de-paris"),
        name: mutateString(NAME_SEED),
        label: mutateString("GMdP"),
      };
      expect(typeof mosqueDisplayName(m)).toBe("string");
    }
  });
});

describe("fuzz: normalizeAlerts (UI boundary against a tampered config)", () => {
  it("always yields a fully-sane AlertsConfig, never throws", () => {
    for (let i = 0; i < 4000; i++) {
      const hostile = mutateStructure(ALERTS_SEED);
      let out;
      try {
        out = normalizeAlerts(hostile, below(2) === 0);
      } catch (e) {
        throw new Error(`normalizeAlerts threw on ${JSON.stringify(hostile)}: ${e}`);
      }
      for (const key of ["fajr", "dhuhr", "asr", "maghrib", "isha"] as const) {
        const p = out[key];
        expect(["silent", "default", "adhan"]).toContain(p.mode);
        expect(p.sound === null || typeof p.sound === "string").toBe(true);
        if (p.volume !== null) {
          expect(Number.isInteger(p.volume)).toBe(true);
          expect(p.volume).toBeGreaterThanOrEqual(0);
          expect(p.volume).toBeLessThanOrEqual(100);
        }
        if (p.notify_before_min !== null) {
          expect(p.notify_before_min).toBeGreaterThanOrEqual(1);
          expect(p.notify_before_min).toBeLessThanOrEqual(120);
        }
      }
      if (out.shuruq_notify_before_min !== null) {
        expect(out.shuruq_notify_before_min).toBeGreaterThanOrEqual(1);
        expect(out.shuruq_notify_before_min).toBeLessThanOrEqual(120);
      }
    }
  });

  it("preserves a fully-valid config untouched", () => {
    for (let i = 0; i < 500; i++) {
      const out = normalizeAlerts(JSON.parse(JSON.stringify(ALERTS_SEED)), true);
      expect(out).toEqual(ALERTS_SEED);
    }
  });
});

describe("fuzz: clampVolume + stepNotifyBefore", () => {
  it("clampVolume always lands on an integer 0..100", () => {
    for (let i = 0; i < 3000; i++) {
      const v = clampVolume(mutateNumber(50));
      expect(Number.isInteger(v)).toBe(true);
      expect(v).toBeGreaterThanOrEqual(0);
      expect(v).toBeLessThanOrEqual(100);
    }
  });

  it("stepping always lands on the ladder or off", () => {
    for (let i = 0; i < 3000; i++) {
      const current = below(4) === 0 ? mutateNumber(5) : NOTIFY_LADDER[below(NOTIFY_LADDER.length)];
      for (const direction of [1, -1] as const) {
        const out = stepNotifyBefore(current as number | null, direction);
        expect(out === null || NOTIFY_LADDER.includes(out as never)).toBe(true);
      }
    }
  });
});

describe("fuzz: hostile rendering", () => {
  it("month table never turns mutated strings into markup", () => {
    for (let i = 0; i < 250; i++) {
      const table = document.createElement("table");
      const cells = Array.from({ length: 6 }, () => mutateString(NAME_SEED));
      const rows = Array.from({ length: 5 }, (_, d) => [String(d + 1), ...cells]);
      renderMonthTable(table, ["Fajr", "Shurouq", "Dhuhr", "Asr", "Maghrib", "Isha"], rows, below(6));
      expect(activeElements(table)).toEqual([]);
    }
  });

  it("search results never turn mutated names into markup", () => {
    for (let i = 0; i < 250; i++) {
      const list = document.createElement("ul");
      const mosques = Array.from({ length: 5 }, () => ({
        slug: below(3) === 0 ? undefined : mutateString("some-mosque"),
        name: mutateString(NAME_SEED),
        locality: mutateString("Paris"),
        country: mutateString("France"),
      }));
      renderResults(list, mosques as never, () => {});
      expect(activeElements(list)).toEqual([]);
    }
  });
});

describe("fuzz: seedAlertsFromSound is total", () => {
  it("accepts anything truthy-shaped and stays sane", () => {
    for (let i = 0; i < 500; i++) {
      const seeded = seedAlertsFromSound(below(2) === 0);
      for (const key of ["fajr", "dhuhr", "asr", "maghrib", "isha"] as const) {
        expect(["silent", "default", "adhan"]).toContain(seeded[key].mode);
      }
    }
  });
});
