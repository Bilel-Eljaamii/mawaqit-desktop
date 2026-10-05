# Changelog

All notable changes to mawaqit-desktop are documented here. Versions follow
[semver](https://semver.org/); the app and the
[mawaqit-api](../mawaqit-api) library version-lock at release time.

## [0.10.0] — 2026-10-05

### Changed
- Version bump only (0.9.0 → 0.10.0).

## [0.9.0] — 2026-10-05

### Added
- **Tor toggle in the topbar** — a shield button flips Tor routing on/off
  immediately: the transport swaps in place, so alarms and data paths pick
  it up without a restart. Gold when on; tooltip shows the proxy.
- **Structured Tor config** — the settings Privacy section now has separate
  host and port fields with the `socks5h://` scheme fixed automatically
  (a DNS-leaking plain `socks5://` can never be stored). Host charset and
  port bounds (1–65535) are enforced at save.
- Legacy `tor_socks_addr` configs migrate into the structured block on
  load (host parsed, port enforced, enabled preserved) — pinned by tests.
- New `ClientHolder` infrastructure: the mawaqit.net client swaps in place
  via `set_tor_enabled` / `update_config`, and the background alarm loop
  re-reads it per cycle.

### Fixed
- An implausible Tor host or unreachable proxy degrades to a direct
  connection with a log line — the app always starts.

## [0.8.1] — 2026-10-04

### Fixed
- Enter in the mosque-dialog search field now does the same as clicking
  Search.
- Live suggestions: after 4 letters the mosque results render as you type
  (300 ms debounce).
- Save Settings always gives visible feedback: a toast on success and a
  toast (instead of the easy-to-miss status line) when the Tor address
  validation fails. Wiring verified by a new happy-dom smoke test that
  executes the real `main.ts` against the real `index.html`.

## [0.8.0] — 2026-10-04

### Added
- **Per-prayer athan voice picker** — an "Athan voice" sheet in the
  notifications panel: Built-in athan pinned first, then muadhin recordings
  from Mawaqit's public CDN (Makkah, Madinah, Al-Aqsa/Qods, Algeria, Egypt +
  Fajr variants) served keylessly from `cdn.mawaqit.net`. Each entry has a
  preview; selecting downloads the file (8 MB cap, atomic write, routed
  through the client's transport — Tor applies). "Custom file…" stays.
- Per-prayer `voice` in the alerts config (survives restarts).
- `adhan_voices` / `voice_is_cached` / `download_voice` IPC commands; the
  panel renders the catalog from Rust (no TS mirror to drift).

### Fixed
- Offline fallback honored exactly as specified: a voice that is not
  downloaded (offline, CDN down) plays the **built-in athan** — a missing
  voice never silences or delays the adhan. A background download retries
  for the next adhan.
- `update_config` rejects unknown voice ids at save time.

## [0.7.0] — 2026-10-03

### Added
- **Tor routing (privacy)** — settings toggle to send all mawaqit.net
  traffic through Tor (any SOCKS5 proxy with remote DNS, `socks5h://`).
  Off by default; applied at startup; an unreachable or strictly-invalid
  proxy degrades to a direct connection with a log line — the app always
  starts. Composes with the offline snapshots: Tor flakiness falls back to
  the stored times instead of erroring.
- Dependency on mawaqit-api 0.3.0 (`with_socks_proxy`, raised timeouts
  when proxied).

## [0.6.0] — 2026-10-03

### Fixed
- The sound dropdown in the notifications panel rendered as a white
  browser-default box: `color-scheme: dark` is now set globally (native
  select popups, checkboxes and scrollbars render dark) and the select is
  solid dark with a custom chevron.

## [0.5.4] — 2026-10-03

### Fixed
- **Ctrl+Alt+P was completely dead** in 0.5.3: a refactor lost the shortcut
  registration and the overlay positioning call sites; only dead_code
  warnings hinted at it. Both restored, and a new `glance_wiring` contract
  test greps the wiring so a silent drop fails CI instead of shipping.
- `STATIC_VCRUNTIME` deprecation: the variable comes from the build
  environment (nothing in the repo sets it); static VC linking is already
  the schema default, so unsetting it removes the warning with no behavior
  change.

## [0.5.3] — 2026-10-03

### Added
- App version shown in the tray menu and the settings dialog footer.

## [0.5.2] — 2026-10-03

### Fixed
- The mosque-announcements dialog could never be closed: its own
  `display: flex` beat the UA's `[hidden] { display: none }`. The hidden
  attribute now always wins (global guard), which also fixed the adhan
  options showing in Silent/Default modes and the Today view stacking
  above the Month tab.

## [0.5.1] — 2026-10-03

### Fixed
- Save Settings always gives visible feedback (toast on success and on
  validation failure); wiring verified by a happy-dom smoke test executing
  the real `main.ts` against the real `index.html`.

## [0.5.0] — 2026-10-03

### Added
- **Mosque announcements inbox** — a message icon with an unread badge;
  the inbox lists the mosque's announcements (title, dates, text) with
  per-item read state persisted locally; "Mark all read" action.
- **Topbar reorganization** — mosque switching moved to its own mosque
  icon; the gear is now all-settings.

### Fixed
- The announce dialog could never be closed (author `display:` beat the
  UA's `[hidden]` rule); a global hidden-attribute guard fixed the same
  latent bug in the adhan options and the Today/Month view stacking.

### Security
- `update_config` rejects unknown voice ids at save time.

## [0.4.0] — 2026-10-03

### Added
- **Desktop-native visibility** — the window title carries the countdown
  (taskbar/alt-tab); the tray menu lists today's times with the upcoming
  prayer marked; the tray icon and tooltip show the offline state.
- In-tree mutation fuzzers for every layer (search model, page_url, disk
  snapshots, config files, alarm engine, TodayPayload IPC, DOM rendering).

### Fixed
- **REDTEAM F4** — surfaced prayer times are always strict `HH:MM`: a row
  carrying a hostile value (`"25:70"`) rejects the whole day instead of
  showing fabricated times; the iqama passthrough falls back to adhan for
  non-display shapes; the frontend `parseHhmmToDate` enforces ranges.
- **REDTEAM F11** (found by the frontend mutation fuzzer) —
  `sanitizeCssUrl` emitted an escaped copy of the *raw* string, so control
  characters the URL parser strips could make CSS fetch a different URL
  than the one validated; it now emits the parsed URL.
- Warning-free release builds.

### Changed
- Settings Save gives visible feedback (toast on success and on
  validation failure).

## [0.3.0] — 2026-10-02

### Added
- **Offline prayer times** — one snapshot per mosque of the whole-year
  confData (adhan + iqama calendars), stored in the app config directory.
  When the network is down the app serves the snapshot, keeps the alarms
  and shows an "Offline — times from …" badge (the tray says so too).
- `mawaqit-api::disk`: hashed filenames defeat hostile slugs, atomic
  writes, total-parse loading (hostile files degrade to "no snapshot").
- `MawaqitClient::with_disk_cache`: failed fetches fall back to the
  snapshot; successful fetches refresh it; `conf_data_dated` reports the
  source.
- Hostile snapshot tests + dead-port integration tests.

### Fixed
- **REDTEAM F10** (found by the Rust mutation fuzzer) — snapshots of
  wire-tolerated confData never reloaded (hostile raw shapes broke strict
  serde): the envelope now stores the tolerant struct's values with
  unmodeled extras overlaid.

## [0.2.0] — 2026-10-02

### Added
- **Per-prayer notification settings** — each prayer configures
  Silent/Default/Adhan independently, with its own sound (built-in or
  custom mp3/wav), custom volume, notify-before (1–30 min, exactly one
  reminder) plus a global pre-Shuruq reminder and "apply to all prayers".
- Notifications panel reorganized: bell icon, per-prayer tabs, voice/sound
  picker, custom volume, preview.
- Autostart hardening: dev runs and AppImage mount paths can no longer
  poison the menu/autostart entries; settings saves only toggle the
  autostart plugin on actual state changes.
- Hostile config tests for the new fields.

### Fixed
- **REDTEAM F9** — a missing `sound_enabled` field no longer wipes the
  whole config (probe un-ignored, now the regression guard).
- **Autostart poisoning** — dev runs and AppImage mount paths can no
  longer write doomed paths into the menu/autostart entries
  (`entry_exec_path` prefers `$APPIMAGE`, refuses `/tmp/.mount_*`).
- Warning-free release builds; toolchain aligned (tauri 2.12.x across
  npm and crates).

### Changed
- The legacy global athan switch stays in the config for downgrades and
  is recomputed from the per-prayer settings on save.

## [0.1.0] — 2026-09-26

### Added
- Initial release: keyless mosque search, today/month views, tray countdown
  with urgency colors, adhan playback with manual stop, iqama alerts,
  autostart to tray, offline snapshot foundation, hostile red-team suite
  (REDTEAM.md), Windows/macOS/Linux/Raspberry Pi cross-build scripts.
