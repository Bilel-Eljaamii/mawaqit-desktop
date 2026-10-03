/** Hostile tests for the announcement inbox helpers: the announcement list
 * comes from mawaqit.net over IPC and the read list from the
 * attacker-writable config file — every helper must be total, order-stable
 * and size-capped. */
import { describe, expect, it } from "vitest";
import {
  MAX_INBOX_ITEMS,
  MAX_READ_ENTRIES,
  markAllRead,
  markRead,
  pruneRead,
  unreadCount,
  unreadItems,
  visibleAnnouncements,
} from "../../src/lib/announcements";
import type { AnnouncementItem } from "../../src/lib/announcements";

function item(id: string): AnnouncementItem {
  return { id, title: `T ${id}`, content: `C ${id}`, start_date: null, end_date: null };
}

describe("visibleAnnouncements", () => {
  it("caps the inbox at MAX_INBOX_ITEMS, keeping wire order", () => {
    const items = Array.from({ length: 5000 }, (_, i) => item(`a${i}`));
    const visible = visibleAnnouncements(items);
    expect(visible).toHaveLength(MAX_INBOX_ITEMS);
    expect(visible[0].id).toBe("a0");
    expect(visible[MAX_INBOX_ITEMS - 1].id).toBe(`a${MAX_INBOX_ITEMS - 1}`);
  });

  it("passes hostile content through untouched (inert text later)", () => {
    const items = [
      {
        id: "<script>alert(1)</script>",
        title: "\u202EesreveR",
        content: '"><img src=x onerror=alert(1)>',
        start_date: null,
        end_date: null,
      },
    ];
    expect(visibleAnnouncements(items)).toEqual(items);
  });
});

describe("unread counting", () => {
  it("counts ids missing from the read list", () => {
    const items = [item("a"), item("b"), item("c")];
    expect(unreadCount(items, [])).toBe(3);
    expect(unreadCount(items, ["a"])).toBe(2);
    expect(unreadCount(items, ["a", "b", "c"])).toBe(0);
    expect(unreadItems(items, ["a"]).map((i) => i.id)).toEqual(["b", "c"]);
  });

  it("is immune to duplicate and weird ids", () => {
    const items = [item("\u0000"), item("\u202Ex"), item("a")];
    expect(unreadCount(items, ["\u0000", "\u202Ex"])).toBe(1);
    expect(unreadCount(items, ["nope", "nope"])).toBe(3);
  });
});

describe("markRead", () => {
  it("appends once and never duplicates", () => {
    expect(markRead([], "a")).toEqual(["a"]);
    expect(markRead(["a"], "a")).toEqual(["a"]);
    expect(markRead(["a", "b"], "c")).toEqual(["a", "b", "c"]);
  });

  it("caps to the newest MAX_READ_ENTRIES marks", () => {
    let read: string[] = [];
    for (let i = 0; i < MAX_READ_ENTRIES + 50; i++) read = markRead(read, `id${i}`);
    expect(read).toHaveLength(MAX_READ_ENTRIES);
    expect(read[0]).toBe("id50");
    expect(read[read.length - 1]).toBe(`id${MAX_READ_ENTRIES + 49}`);
  });
});

describe("markAllRead", () => {
  it("marks the FULL list — the inbox cap is render-only", () => {
    const items = Array.from({ length: 300 }, (_, i) => item(`a${i}`));
    const read = markAllRead(items, []);
    expect(unreadCount(items, read)).toBe(0);
    expect(read).toContain("a0");
    expect(read).toContain("a299");
  });

  it("respects the read-list cap with enormous lists", () => {
    const items = Array.from({ length: MAX_READ_ENTRIES + 100 }, (_, i) => item(`a${i}`));
    const read = markAllRead(items, []);
    expect(read).toHaveLength(MAX_READ_ENTRIES);
  });
});

describe("pruneRead", () => {
  it("keeps only marks matching current announcements", () => {
    const items = [item("a"), item("b")];
    expect(pruneRead(items, ["a", "ghost", "b"])).toEqual(["a", "b"]);
    expect(pruneRead([], ["a"])).toEqual([]);
  });
});

// ---------------------------------------------------------------- css guard

import { readFileSync } from "node:fs";

describe("stylesheet hidden-guard (regression: unclosable announce dialog)", () => {
  it("forces the hidden attribute over author display rules", () => {
    // v0.5.0 shipped #announce-dialog { display: flex }, whose author rule
    // beat the UA's [hidden] { display: none } — the dialog could never be
    // closed. The guard rule must stay present for EVERY hidden-toggled
    // element (announce dialog, adhan options, the Today view).
    const css = readFileSync("src/styles.css", "utf8"); // vitest runs from the project root
    expect(css).toMatch(/\[hidden\]\s*\{[^}]*display:\s*none\s*!important[^}]*\}/);
  });
});
