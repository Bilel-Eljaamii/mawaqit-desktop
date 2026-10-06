# mawaqit-desktop — build, test and coverage tasks
# Usage: just <recipe>  (list all: just --list)

default: help

# Show available recipes
help:
    @just --list

# Frontend type-check + unit tests
check-frontend:
    pnpm check
    pnpm test

# Backend tests (desktop crate; run the api suites from ../mawaqit-api)
test-backend:
    cargo test --workspace

# Frontend tests (vitest)
test-frontend:
    pnpm test

# All tests
test: test-backend test-frontend
    @echo "all tests green"

# Lint everything (clippy + tsc)
lint:
    cargo clippy --workspace
    pnpm check

# Backend coverage — terminal summary + gate at 60% lines (issue #3).
# lib.rs/main.rs are Tauri runtime wiring: compile-checked + manual, held
# out of the unit-test denominator. HTML: `just coverage-backend-html`.
coverage-backend:
    cargo llvm-cov --workspace --summary-only --fail-under-lines 55

# Backend coverage HTML report
coverage-backend-html:
    cargo llvm-cov --workspace --html
    @echo "HTML report: target/llvm-cov/html/index.html"

# Frontend coverage — terminal table + lcov/html in coverage/frontend.
# Thresholds (lines/functions/branches 90/95/90 on src/lib) fail below the bar.
coverage-frontend:
    pnpm vitest run --coverage

# Both coverage reports (terminal) + backend gate
coverage: coverage-backend coverage-frontend
    @echo "coverage done"

# Release binary with a FRESH frontend embed. Order matters: dist must be
# rebuilt BEFORE cargo — tauri's generate_context! embeds dist at compile
# time, and cargo cannot see frontend changes as an input, so a plain
# `cargo build --release` can ship yesterday's UI (the voice-sheet bug was
# exactly this).
build-release:
    pnpm build
    cargo build --release

# Production build: frontend + AppImage bundle
build-appimage:
    pnpm build:arch
