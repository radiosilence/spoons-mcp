# spoons-mcp — Wetherspoon's pubs, menus and prices from the order-and-pay API

# What CI checks: the workflows, format, lint with warnings as errors, tests, and the MCP App's types and build.
check:
    actionlint
    git diff --quiet Cargo.lock || { echo 'Cargo.lock has uncommitted changes'; exit 1; }
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo test --workspace --locked
    just app-fresh

# The checks that take seconds, run before every push; CI runs the rest.
fast:
    git diff --quiet Cargo.lock || { echo 'Cargo.lock has uncommitted changes'; exit 1; }
    cargo fmt --all -- --check

# Build the MCP App (crates/spoons-mcp/app) into assets/app.html, the one file the server embeds. It's committed, so
# a Rust build needs no Node; CI fails a change where it's stale.
app:
    cd crates/spoons-mcp/app && aube install --frozen-lockfile && aube run check && aube run build

# Fail when the committed app isn't what its sources build, or they don't typecheck.
app-fresh:
    #!/usr/bin/env bash
    set -euo pipefail
    cd crates/spoons-mcp/app
    aube install --frozen-lockfile
    aube run check
    out=$(mktemp -d)
    trap 'rm -rf "$out"' EXIT
    npx vite build --logLevel error --outDir "$out"
    cmp -s "$out/app.html" ../assets/app.html || { echo 'assets/app.html is stale: run `just app`'; exit 1; }
    echo "app.html: $(gzip -9c ../assets/app.html | wc -c) bytes gzipped"

# Drive the app in the ext-apps reference host against fixtures, with screenshots into app/e2e/out. Needs the host
# built from modelcontextprotocol/ext-apps (examples/basic-host) at HOST_DIR or app/e2e/host, and Playwright.
app-e2e:
    cargo build
    cd crates/spoons-mcp/app && node e2e/run.mjs

# Format
fmt:
    cargo fmt --all

# Install the git hooks
hooks:
    lefthook install

# Write CHANGELOG.md from the fragments in changelog.d/. `--stdout` prints it instead.
changelog *args:
    python3 scripts/changelog.py {{args}}

# Open the release PR: `just release patch|minor|major`, or an exact version.
# Merging it publishes the GitHub release with the binaries; tags are never pushed by hand.
release version *args:
    scripts/release.sh {{version}} {{args}}
