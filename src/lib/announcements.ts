// Pure mosque-announcement inbox helpers, extracted for direct testing.
// Announcement titles/contents/ids come from mawaqit.net over IPC and the
// read-list comes from the attacker-writable config file: every function
// here is total (never throws), order-stable, and size-capped.

/** Mirrors the Rust `AnnouncementDto` serde struct. */
export interface AnnouncementItem {
  id: string;
  title: string | null;
  content: string | null;
  start_date: string | null;
  end_date: string | null;
}

/** The inbox never renders more than this many items — a hostile page with
 * thousands of announcements must not hang the DOM. */
export const MAX_INBOX_ITEMS = 50;

/** The persisted read-list is capped (backend clamps on save too). */
export const MAX_READ_ENTRIES = 500;

/** The first MAX_INBOX_ITEMS announcements, in wire order. */
export function visibleAnnouncements(items: AnnouncementItem[]): AnnouncementItem[] {
  return items.slice(0, MAX_INBOX_ITEMS);
}

/** Items the user has not read yet (id not in the read list). */
export function unreadItems(items: AnnouncementItem[], read: string[]): AnnouncementItem[] {
  const seen = new Set(read);
  return items.filter((a) => !seen.has(a.id));
}

export function unreadCount(items: AnnouncementItem[], read: string[]): number {
  return unreadItems(items, read).length;
}

/** Mark one announcement read: order-stable append, capped to the newest
 * MAX_READ_ENTRIES marks (oldest dropped). Already-read ids stay put. */
export function markRead(read: string[], id: string): string[] {
  if (read.includes(id)) return read;
  const next = [...read, id];
  return next.length > MAX_READ_ENTRIES ? next.slice(next.length - MAX_READ_ENTRIES) : next;
}

/** Mark every currently-visible announcement read, capped like markRead. */
export function markAllRead(items: AnnouncementItem[], read: string[]): string[] {
  const seen = new Set(read);
  const next = [...read];
  for (const a of visibleAnnouncements(items)) {
    if (!seen.has(a.id)) next.push(a.id);
  }
  return next.length > MAX_READ_ENTRIES ? next.slice(next.length - MAX_READ_ENTRIES) : next;
}

/** Drop read marks that no longer match any current announcement, capped —
 * the mosque's current list is the source of truth for what can be unread. */
export function pruneRead(items: AnnouncementItem[], read: string[]): string[] {
  const current = new Set(items.map((a) => a.id));
  const kept = read.filter((id) => current.has(id));
  return kept.length > MAX_READ_ENTRIES ? kept.slice(kept.length - MAX_READ_ENTRIES) : kept;
}
