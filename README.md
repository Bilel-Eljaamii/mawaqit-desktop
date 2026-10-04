# Mawaqit Desktop

Prayer times from your mosque, right in your system tray — **no account required**.

Mawaqit Desktop is a small cross-platform desktop app built on [mawaqit.net](https://mawaqit.net)'s
public data (Tauri 2 — Rust backend, vanilla TypeScript frontend). Search for your mosque once,
and the app lives in your tray: it counts down to the next prayer, raises a notification when the
adhan starts, plays the athan, and can remind you at iqama time too.

## Repository layout

Two repositories, by design: this app repo, and the
**[mawaqit-api](../mawaqit-api)** client library — split out so the community
can build their own apps on the same keyless data layer (and so the library
can be published to crates.io on its own cadence). The app imports it as a
path dependency pinned to the library's version:

```toml
mawaqit-api = { version = "0.2.0", path = "../../mawaqit-api" }
```

| Path | What it is |
|---|---|
| `src/`, `index.html` | Frontend (vanilla TypeScript + Vite), rendered by Tauri |
| `src-tauri/` | Desktop app crate (domain / application / infrastructure / presentation) |
| `../mawaqit-api` | Keyless Rust client library for mawaqit.net (separate repo: src, hostile suites, fuzz, examples) |
| *(topbar)* | 🕌 switch mosque · 🔔 per-prayer notifications · ✉ mosque announcements (unread badge) · ⚙ settings |
| `tests/frontend/` | Vitest hostile-rendering suite |
| `docs/` | Product material (deck, design notes) |
| `scripts/` | Build helpers (cross-compile wrappers) |

## Features

- **Keyless** — uses mawaqit.net's public mosque-search endpoint and the data embedded in each
  mosque's public page. No login, no API key, nothing personal stored or sent.
- **Mosque announcements inbox** — when your mosque publishes announcements on mawaqit.net,
  a message icon shows the number you haven't read; the inbox lists them (title, dates, text)
  and per-item read state is stored locally. Text only by design — no wire images or videos.
- **Quick glance anywhere** — a global shortcut (`Ctrl+Alt+P`) pops a compact
  frameless overlay with today's times and the live countdown; it dismisses
  itself on focus loss, with Escape or its own close button (a shortcut that
  can't be registered — another app owning it — degrades to no shortcut,
  never blocks the app). The tray menu also lists today's times (next prayer
  marked), and the window title carries the countdown for alt-tab/taskbar.
- **Works offline** — every successful fetch stores a snapshot of the mosque's whole year
  (adhan + iqama calendars) in the app config directory; when the network is down the app
  serves the snapshot, keeps the alarms and shows an "Offline — times from …" badge (the tray
  says so too). Without a snapshot the app behaves as before and retries.
- **Tray countdown** — the tray icon carries a small countdown badge (color-coded as the prayer
  approaches) and a tooltip with the exact time; the badge gets out of the way when the next
  prayer is more than an hour and a half away.
- **Athan with manual stop** — at adhan time: desktop notification + athan audio. Stop it from the
  tray menu, from the button that appears in the app, or from the notification itself (click or
  button on Linux and Windows; on macOS the tray item is the stop path).
- **Optional iqama alerts** — notifications when each iqama starts; the tray then counts down to
  iqama instead of adhan.
- **Native UI** — today view with live countdown and per-prayer cards (adhan + iqama), month
  calendar with an adhan/iqama toggle, Shurouq and Jumu'a chips, the mosque's own photo as
  backdrop — mirroring the mawaqit.net display page.
- **Mosque search** — by keyword, powered by the public search endpoint (coordinates are also
  supported in the crate).
- **Autostart & close-to-tray** — starts minimized to the tray at login if enabled; closing the
  window keeps the app running; quit from the tray menu.
- **Robust parsing** — handles all known mosque data layouts (standard and "Sabah İmsak" /
  Diyanet-style calendars, relative `+N` iqama offsets), with fuzzing and a 100+ mosque
  live-tour test suite (see [Testing](#testing)).

## Getting started

### Prerequisites

- [Rust](https://rustup.rs) (stable) and [pnpm](https://pnpm.io) (npm also works for the frontend)
- Linux: the usual [Tauri 2 dependencies](https://tauri.app/start/prerequisites/), e.g. on
  Debian/Ubuntu: `libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev
  libayatana-appindicator3-dev librsvg2-dev`

### Run in development

```sh
pnpm install
pnpm tauri dev        # or: pnpm dev:app
```

### Build for production

```sh
pnpm build:linux      # .deb + .rpm + AppImage  (see package.json for Windows/macOS/Pi targets)
pnpm build:arch       # AppImage only
```

Artifacts land in `target/release/bundle/`.

### Windows from Linux (cross-compile)

Building the Windows exe on a Linux host works through [cargo-xwin](https://github.com/rustcross/cargo-xwin)
(MSVC target + the Windows SDK downloaded automatically, ~1 GB on first run):

```sh
rustup target add x86_64-pc-windows-msvc
cargo install --locked cargo-xwin
pnpm build:windows:cross
```

This produces the portable `target/x86_64-pc-windows-msvc/release/mawaqit_desktop.exe`
(Windows 10/11 ship the WebView2 runtime it needs). The NSIS `*-setup.exe` installer
step additionally requires `wine` (`sudo pacman -S wine` on Arch/Manjaro) to run the
Windows makensis; without wine the exe still builds but the installer step fails.
For signed, dependency-free installers without local tooling, push a `v*` tag —
`.github/workflows/release.yml` builds Windows (MSI + NSIS), Linux and macOS
artifacts on real CI runners.

> **Note:** a binary built without Tauri's `custom-protocol` feature (e.g. a plain `cargo build`)
> expects the Vite dev server on `localhost:1420` and shows *"Could not connect to localhost"*
> when it isn't running. `pnpm tauri build` enables the feature automatically; for a quick
> standalone debug binary use `cargo build --features custom-protocol` inside `src-tauri/`.

### First run

The onboarding screen asks for one thing: find your mosque (type a city or name, pick it from the
results). Everything else — tray, alerts, autostart, athan sound, iqama notifications — is in the
gear-icon settings.

## The `mawaqit-api` library

All mawaqit.net access lives in the separate `mawaqit-api` repository
(clone it as a sibling of this repo: `~/Workspace/mawaqit-api`), reusable
without the desktop app:

```rust
use mawaqit_api::MawaqitClient;

let client = MawaqitClient::new();
let mosques = client.search_mosques("Paris").await?;
let slug = mosques[0].mosque_id().unwrap(); // e.g. "grande-mosquee-de-paris"

let today = client.today(slug).await?;      // adhan + iqama resolved for today
println!("Fajr {} (iqama {})", today.adhan.fajr,
    today.iqama.as_ref().map(|i| i.fajr.as_str()).unwrap_or("?"));
```

Handy CLI check for any mosque (prints exactly what the client fetches):

```sh
cd ../mawaqit-api && cargo run --example times -- grande-mosquee-de-paris
```

## Testing

```sh
# App repo — Rust (desktop) + frontend tests:
cargo test --workspace            # desktop crate: unit + adversarial + mock-server tests (offline)
pnpm test                         # frontend tests (vitest)

# Library repo — its own full hostile suite (offline):
cd ../mawaqit-api && cargo test

# Live checks (hit mawaqit.net, ignored by default, run in the library repo):
cargo test -- --ignored --nocapture                                   # one mosque, end to end
cargo test --test world_hostile -- --ignored --nocapture              # 131 mosques, 5 continents
```

The world tour asserts structural invariants for every mosque (valid times, complete calendars,
resolved iqama); the corpus and fuzz targets throw adversarial input at the page parser and
calendar pipeline; `hostile_http` exercises the client against a local mock server.

## Data source & privacy

- Mosques are identified by their mawaqit.net page slug (e.g. `grande-mosquee-de-paris`).
- All data comes from two public surfaces: `GET /api/2.0/mosque/search?word=…` and the mosque
  page itself, whose embedded `confData` object carries the daily times, the year adhan calendar,
  the iqama calendar, Jumu'a times and the mosque photo. One fetched page is cached in memory
  (6 h) and shared by every feature.
- The app stores only your mosque choice and toggles in
  `~/.config/mawaqit-desktop/mawaqit-config.json` — no credentials, ever.
- This project is an independent client and is not affiliated with Mawaqit. Prayer times are only
  as accurate as what each mosque's admins publish there.
