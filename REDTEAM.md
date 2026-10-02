# REDTEAM.md — hostile-data testing for mawaqit-desktop

The app consumes everything it displays from the internet without accounts or
keys: mosque search results, mosque pages (the embedded `confData` blob with
daily times, the year calendar, iqama offsets, the mosque name, jumua strings
and the background image URL), all flowing through `mawaqit-api` → Tauri IPC →
`src/main.ts`. On top of that, the config file is writable by any process
running as the user. This suite treats **every one of those inputs as
attacker-controlled** and pins what the app must do about it.

## How to run

```sh
# fast, offline, CI-safe (all contract tests — must be green)
cargo test --workspace
pnpm test

# the red-team finding probes (each deliberately FAILS until its finding
# is fixed — see the findings table below; offline, fast)
cargo test -p mawaqit-api --no-fail-fast --test hostile_http --test hostile_semantics -- --ignored
cargo test -p mawaqit_desktop --no-fail-fast --lib --test config_hardening -- --ignored

# live-site campaigns (slow, online)
cargo test -p mawaqit-api --test world_hostile -- --ignored --nocapture
cd mawaqit-api/fuzz && cargo fuzz run parse_page   # nightly toolchain
```

## Suite map

| File | Layer | What it attacks |
|---|---|---|
| `mawaqit-api/tests/hostile_http.rs` | HTTP client | crafted server responses: garbage/oversized/non-UTF8 bodies, redirects, truncation, hostile slugs, CRLF in search words |
| `mawaqit-api/tests/hostile_semantics.rs` | parser + calendar | valid JSON that lies: out-of-range times, duplicate day keys, layout confusion, display-field spoofing |
| `mawaqit-api/tests/hostile_corpus.rs` | parser | byte-level torture of the confData extraction (pre-existing) |
| `mawaqit-api/tests/world_hostile.rs` | end-to-end | 100+ real mosques across five continents (pre-existing) |
| `src-tauri/src/infrastructure/config.rs` (`#[cfg(test)]`) | persistence | tampered config files: garbage, wrong types, hostile slugs, legacy formats |
| `src-tauri/src/application/prayer_logic.rs` (`#[cfg(test)]`) | alarms | hostile time strings reaching the adhan/iqama alert engine |
| `src-tauri/tests/config_hardening.rs` | build config | CSP, `withGlobalTauri`, capability minimality |
| `tests/frontend/hostile-display.test.ts` | webview rendering | XSS payload zoo through every rendering path; CSS URL sanitizer breakouts; countdown rollover |
| `tests/frontend/notifications.test.ts` | alerts settings | hostile IPC alerts blocks (wrong types, bad enums, clamped numbers, XSS sound paths); stepper/clamp/apply-to-all contracts |
| `mawaqit-api/src/disk.rs` (`#[cfg(test)]`) + `tests/disk_cache.rs` | offline snapshots | attacker-writable snapshot files: garbage/truncated/wrong-slug/wrong-version degrade to "no snapshot"; hostile slugs can't escape the cache dir (hashed filenames); served-from-snapshot behavior pinned end-to-end |
| `mawaqit-api/fuzz/fuzz_targets/` | parser | libFuzzer campaigns beyond the pinned corpus (pre-existing) |

Test-enabling seams added (no behavior change for production callers):
`MawaqitClient::with_base_urls()` + `mawaqit_api::page_url()` (point the
client at a local mock server / assert URL construction), `load_config_from()`
(load configs from a test path), and `src/lib/display.ts` (pure display
helpers extracted from `main.ts` for direct testing).

## Findings

Each open finding is an `#[ignore]`d Rust test (or `test.fails` in vitest)
that currently **fails on purpose** — the failing assertion is the evidence.
Fix the code, un-ignore the test, and it becomes the regression guard.

| ID | Severity | Finding | Probe |
|---|---|---|---|
| F1 | High | **Cross-origin redirects are followed.** One open redirect (or a compromise) on mawaqit.net turns `conf_data` into "parse whatever the redirect target serves" — attacker-authored prayer times, mosque name and image URL on the user's screen. Fix: pin `reqwest`'s redirect policy to same-host (or `Policy::none`). | `hostile_http::finding_f1_cross_origin_redirect_is_not_followed` |
| F2 | High | **Slugs are never validated.** Search-result slugs flow verbatim: search → `update_config` → config file → `https://mawaqit.net/en/{slug}`. `../` escapes the mosque namespace (verified: `../trap` requests `/trap` and its content is served as the mosque), `?`/`#` swap the page under a legit-looking slug, and a tampered config file persists any of it forever. Fix: validate once at the `Mosque::mosque_id()` boundary and in `update_config` (e.g. `^[a-z0-9]+(-[a-z0-9]+)*$`). | `hostile_http::finding_f2_…`, `config::tests::hostile_slug_…` (documents current verbatim loading) |
| F3 | Medium | **The 20 MB cap buffers before it checks.** `read_capped` calls `response.bytes()`, which downloads the whole body first — a hostile server streaming gigabytes OOMs the app before the cap trips. Fix: stream via `bytes_stream()` and abort once the running total exceeds the cap. | `hostile_http::finding_f3_documented_cap_…` (passes; documents the cap) |
| F4 | Medium | **Adhan times are never validated.** `daily_from_row` passes strings through: `"25:70"` reaches the UI and the frontend `setHours(25, 70)` rolls it into another day, pointing the countdown at a made-up time. The iqama path already falls back to the adhan on garbage — give the adhan path the same treatment. | `hostile_semantics::finding_f4_…`, vitest `F4 (frontend)` |
| F5 | Low | **Duplicate day keys are not deduplicated.** Rust's integer parse accepts `"01"` and `"+1"`, so a hostile month with `1`/`01`/`+1` yields day 1 three times; `times_for_date` silently picks whichever sorts first. Fix: dedupe by parsed day in `month_times`. | `hostile_semantics::finding_f5_…` |
| F6 | Low | **Control/bidi characters reach display strings.** `U+202E` and friends flow into the window title, tray tooltip and OS notifications (`Mawaqit: <name>`), enabling visual spoofing of app state. Fix: strip C0/C1 + bidi controls at the `ConfData` boundary. | `hostile_semantics::finding_f6_…` |
| F7 | — | **CSP was null.** Fixed during this engagement (a real policy now exists in `tauri.conf.json`); the former finding test is now the always-run contract `config_hardening::csp_is_configured`. | — |
| F8 | Medium | **`withGlobalTauri: true` with no consumer.** The frontend imports bundled `@tauri-apps/api` modules; the global `window.__TAURI__` only serves any script that manages to run in the webview, handing it every IPC command including `update_config`. Fix: set `"withGlobalTauri": false`. | `config_hardening::finding_f8_…` |
| F9 | Medium | **One missing config field wipes the whole config.** `sound_enabled` is the only `AppConfig` field without `#[serde(default)]`, so a config carrying the mosque but missing that bool fails to parse entirely and silently resets the app to "no mosque" (trivially reachable by a truncated write or tamper). Fix: `#[serde(default = "default_true")]`. **FIXED in v0.2.0** (the per-prayer alerts block landed with serde defaults on every field, and `sound_enabled` got the same treatment); probe un-ignored and now the regression guard. | `config::tests::finding_f9_partial_config_…` (now always-run) |

Notes (no test): `tauri-plugin-opener` and its `opener:default` capability are
granted but the frontend never calls it — consider dropping the dependency to
shrink the IPC surface. Search results are rendered unbounded: a hostile
response with tens of thousands of entries renders that many `<li>` elements
(bounded by the 20 MB cap, so this is a sluggishness, not a crash).

Since v0.2.0 the per-prayer alerts config can name a `sound` path on disk;
a tampered config file can therefore make the app play any local mp3/wav as
the athan. Bounded by design: the path is opened as audio input only
(missing file / directory / junk bytes fail the decode cleanly — pinned in
`infrastructure::audio::tests`), it never reaches the webview except as
inert `textContent`, and it grants no read beyond "play bytes through the
audio device". The dialog capability added for the picker is read-only
(`dialog:allow-open`).

## Hardened by design (pinned, don't regress)

- Rendering uses `textContent` everywhere; hostile payloads land as inert,
  lossless text (vitest DOM suite proves it payload-by-payload).
- `sanitizeCssUrl` rejects every non-http(s) scheme (including control-char
  scheme smuggling that the URL parser would normalize) and percent-encodes
  every CSS string-breakout character before the mosque image enters
  `url("…")`.
- The HTTP client caps response size, enforces connect/request timeouts, and
  rejects non-UTF8 bodies; search words are percent-encoded by reqwest (CRLF
  smuggling provably never reaches the wire).
- The parser refuses pages without `confData`, without ≥5 time strings or
  without a calendar; calendar rows that don't fit a known layout are skipped
  and the day errors out instead of showing fabricated times.
- Iqama `"+N"` offsets are clamped so `+9223372036854775807` resolves to a
  valid time instead of panicking (the original red-team find, kept in
  `calendar.rs::hostile_iqama_offsets_do_not_panic`).
- The alert engine treats unparsable times as inert (no alarm) and lenient
  parses land on the face-value time — hostile strings can never shift an
  adhan alert.
- Hostile config files (binary junk, wrong types, legacy formats, 4 MB slugs)
  always load to a sane state without panicking. The offline snapshot files in
  `times-cache/` are attacker-writable inputs under the same contract: any
  parse failure degrades to "no snapshot", and the filename is a hash of the
  slug so a hostile slug can never escape the directory.
