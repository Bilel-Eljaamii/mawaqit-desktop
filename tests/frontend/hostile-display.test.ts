/**
 * Hostile rendering tests: every string here is attacker-controlled. It
 * arrives from mawaqit.net over the keyless API (mosque names, slugs,
 * localities, countries, jumua strings, image URLs, prayer times) and must
 * land in the DOM as inert text — never as markup, never as a URL scheme
 * with side effects.
 *
 * Conventions:
 * - plain tests pin contracts that must hold (they guard regressions);
 * - `test.fails("RED TEAM FINDING …")` marks a secure contract the code does
 *   NOT satisfy yet — each is a finding in REDTEAM.md; remove the `.fails`
 *   when the fix lands and the assertion becomes the regression guard.
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
import type { Mosque } from "../../src/lib/display";

// Payloads drawn from real XSS/filter-evasion practice, plus bidi/control
// spoofs aimed at titles and trays.
const XSS_PAYLOADS = [
  "<script>alert(1)</script>",
  "<img src=x onerror=alert(1)>",
  "<svg onload=alert(1)>",
  "<iframe src=javascript:alert(1)></iframe>",
  "</td><script>alert(1)</script>",
  "<<script>script>alert(1)</script>",
  "<a href=\"javascript:alert(1)\">click</a>",
  "javascript:alert(1)",
  "data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==",
  "'\"`}{);</style></script>",
  "\u{202E}esreveR\u{202D} name",       // bidi spoof of the displayed name
  "mosque\u{0000}\u{2028}\u{2029}",     // NUL + line/paragraph separators
  "ড়িক\u{200B}zero\u{FEFF}width",       // invisible characters
  "📚🕌".repeat(50),                    // emoji flood
  "A".repeat(100_000),                  // giant cell
];

function hostileMosque() {
  return {
    slug: "grande-mosquee-de-paris",
    name: XSS_PAYLOADS[0],
    label: XSS_PAYLOADS[1],
    locality: XSS_PAYLOADS[2],
    country: XSS_PAYLOADS[3],
  };
}

function activeElements(root: Element): Element[] {
  return Array.from(root.querySelectorAll("script, img, iframe, svg, object, embed, link, a[href^='javascript:']"));
}

// ---------------------------------------------------------------- sanitizer

describe("sanitizeCssUrl", () => {
  it("keeps plain https urls", () => {
    expect(sanitizeCssUrl("https://pics.test/mosque.jpg")).toBe("https://pics.test/mosque.jpg");
    expect(sanitizeCssUrl("http://pics.test/mosque.jpg")).toBe("http://pics.test/mosque.jpg");
  });

  it("rejects every non-http scheme", () => {
    const hostile = [
      "javascript:alert(1)",
      "JaVaScRiPt:alert(1)",
      "data:image/svg+xml,<svg onload=alert(1)>",
      "data:text/html,<script>alert(1)</script>",
      "vbscript:msgbox(1)",
      "file:///etc/passwd",
      "chrome://settings",
      "ftp://host/file",
      "about:blank",
      // control characters are stripped by the URL parser before the scheme
      // is read — the result must still be rejected:
      "java\u0000script:alert(1)",
      "\u0001javascript:alert(1)",
      "jav\u0009ascript:alert(1)", // tab inside the scheme
      "jav\u000Aascript:alert(1)", // newline inside the scheme
    ];
    for (const url of hostile) {
      expect(sanitizeCssUrl(url)).toBeNull(`must reject ${JSON.stringify(url)}`);
    }
  });

  it("rejects unparsable input", () => {
    for (const url of ["", "   ", "not a url", "://", "https://", "http://" + "\u0000".repeat(10)]) {
      expect(sanitizeCssUrl(url)).toBeNull(`must reject ${JSON.stringify(url)}`);
    }
  });

  it("escapes every css string-breakout character", () => {
    // The result goes into url("…") — quotes, backslashes, parens and
    // whitespace must all be percent-encoded so nothing can leave the
    // string or close the url() call.
    const tricky = [
      'https://pics.test/a".jpg',
      "https://pics.test/a'.jpg",
      "https://pics.test/a\\b.jpg",
      "https://pics.test/a(b).jpg",
      "https://pics.test/a b.jpg",
      "https://pics.test/a\nb.jpg",
      "https://pics.test/a)b(c'd\"e\\f.jpg",
    ];
    for (const url of tricky) {
      const out = sanitizeCssUrl(url);
      expect(out).not.toBeNull();
      const escaped = out as string;
      expect(escaped).toMatch(/^[a-z]+:/);
      for (const ch of ['"', "'", "\\", "(", ")", " ", "\n", "\r", "\t"]) {
        expect(escaped.includes(ch)).toBe(false, `${JSON.stringify(ch)} must be escaped in ${JSON.stringify(escaped)}`);
      }
    }
  });

  it("survives absurdly long urls", () => {
    // A hostile server can ship a 1 MB image URL (bounded by the 20 MB
    // response cap). Sanitizing must not choke on it.
    const long = "https://pics.test/" + "a".repeat(1_000_000);
    expect(sanitizeCssUrl(long)).not.toBeNull();
    expect(sanitizeCssUrl(long)?.length).toBe(long.length);
  });
});

// ------------------------------------------------------------ time parsing

describe("parseHhmmToDate", () => {
  it("parses valid times and clamps to the requested day", () => {
    const d = parseHhmmToDate("05:27");
    expect(d).not.toBeNull();
    expect((d as Date).getHours()).toBe(5);
    expect((d as Date).getMinutes()).toBe(27);
    expect((d as Date).getSeconds()).toBe(0);
    expect((d as Date).getMilliseconds()).toBe(0);

    const d2 = parseHhmmToDate("00:00", 1);
    expect(d2).not.toBeNull();
  });

  it("rejects garbage strings", () => {
    for (const t of ["", "bogus", "25", "05:", ":27", "aa:bb", "05:2x", "+30", "١٣:٠٠", "05:27:00"]) {
      expect(parseHhmmToDate(t)).toBeNull(`must reject ${JSON.stringify(t)}`);
    }
  });

  test.fails("RED TEAM FINDING F4 (frontend): out-of-range times are rejected, not rolled over", () => {
    // Today "25:70" silently becomes 02:10 two days ahead via Date
    // rollover, so a hostile calendar can point the countdown at a made-up
    // time. The secure contract: anything outside 00:00–23:59 is null.
    for (const t of ["25:70", "99:99", "24:00", "23:60"]) {
      expect(parseHhmmToDate(t)).toBeNull(`${t} must be rejected`);
    }
  });
});

// ---------------------------------------------------------------- countdown

describe("formatCountdown", () => {
  it("formats and never shows negative time", () => {
    expect(formatCountdown(0)).toBe("00:00");
    expect(formatCountdown(-60_000)).toBe("00:00");
    expect(formatCountdown(65_000)).toBe("01:05");
    expect(formatCountdown(3 * 3600_000 + 65_000)).toBe("3:01:05");
  });
});

// --------------------------------------------------------------- name logic

describe("mosqueDisplayName", () => {
  it("falls back through label/name/slug", () => {
    expect(mosqueDisplayName(hostileMosque())).toBe(XSS_PAYLOADS[1]);
    expect(mosqueDisplayName({ ...hostileMosque(), label: null })).toBe(XSS_PAYLOADS[0]);
    expect(mosqueDisplayName({ ...hostileMosque(), label: null, name: null })).toBe("grande-mosquee-de-paris");
    expect(mosqueDisplayName({})).toBe("?");
  });
});

// ------------------------------------------------------- hostile rendering

describe("renderMonthTable with hostile cells", () => {
  it("lands every payload as inert text, never as markup", () => {
    const table = document.createElement("table");
    const rows = XSS_PAYLOADS.map((p, i) => [String(i + 1), p, p, p, p, p, p]);
    renderMonthTable(table, ["Fajr", "Shurouq", "Dhuhr", "Asr", "Maghrib", "Isha"], rows, null);

    expect(activeElements(table)).toEqual([]);
    const cells = Array.from(table.querySelectorAll("td"));
    expect(cells).toHaveLength(XSS_PAYLOADS.length * 7);
    // textContent round-trips each payload (inert, but lossless). DOM
    // implementations may normalize NUL, so compare NUL-stripped.
    const normalize = (s: string | null) => (s ?? "").replaceAll("\u0000", "").replaceAll("\uFFFD", "");
    let i = 0;
    for (const td of cells) {
      const payload = rows[Math.floor(i / 7)][i % 7];
      expect(normalize(td.textContent)).toBe(normalize(payload));
      i += 1;
    }
    // markup must not appear anywhere: it is entity-escaped or absent
    expect(table.querySelectorAll("script, img, svg, iframe").length).toBe(0);
  });

  it("keeps 31-day hostile calendars bounded and fast", () => {
    const table = document.createElement("table");
    const row = ["25:70", "<script>x</script>", "99:99", "06:30", "13:00", "19:15", "21:00"];
    const rows = Array.from({ length: 31 }, (_, i) => [String(i + 1), ...row]);
    const start = performance.now();
    renderMonthTable(table, ["Fajr", "Shurouq", "Dhuhr", "Asr", "Maghrib", "Isha"], rows, 3);
    expect(performance.now() - start).toBeLessThan(1000);
    expect(activeElements(table)).toEqual([]);
    const highlighted = table.querySelector("tr.today-row");
    expect(highlighted).not.toBeNull();
  });
});

describe("renderResults with hostile mosque entries", () => {
  it("lands hostile names as inert text and filters slug-less entries", () => {
    const list = document.createElement("ul");
    const picked: Mosque[] = [];
    const mosques = [
      hostileMosque(),
      { name: XSS_PAYLOADS[4] }, // no slug -> filtered out
      { slug: "clean", name: "Clean Mosque", locality: "Paris", country: "France" },
    ];
    renderResults(list, mosques, (m) => picked.push(m));

    expect(activeElements(list)).toEqual([]);
    expect(list.querySelectorAll("li")).toHaveLength(2); // slug-less dropped
    expect(list.querySelectorAll("li.empty")).toHaveLength(0);

    list.querySelectorAll("li")[0].dispatchEvent(new Event("click"));
    expect(picked).toHaveLength(1);
    expect(picked[0].slug).toBe("grande-mosquee-de-paris");
  });

  it("shows the empty-state for hostile-but-unusable results", () => {
    const list = document.createElement("ul");
    renderResults(list, [{ name: XSS_PAYLOADS[5] }, {}, { slug: null, name: "x" }], () => {});
    expect(list.querySelectorAll("li.empty")).toHaveLength(1);
    expect(activeElements(list)).toEqual([]);
  });
});
