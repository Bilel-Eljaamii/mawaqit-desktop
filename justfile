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

# Backend coverage (HTML + terminal). Needs cargo-llvm-cov:
#   cargo install cargo-llvm-cov
coverage-backend:
    cargo llvm-cov --workspace --html
    @echo "HTML report: target/llvm-cov/html/index.html"

# Frontend coverage (v8 provider, terminal + lcov in coverage/)
coverage-frontend:
    pnpm vitest run --coverage

# Both coverage reports
coverage: coverage-backend coverage-frontend
    @echo "coverage done"

# Production build: frontend + AppImage bundle
build-appimage:
    pnpm build:arch
