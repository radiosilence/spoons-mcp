# Changelog

## 0.3.0

### Added

- **`pay` can enter a saved card.** Over HTTP, a card in the `X-Spoons-Card-*` headers is entered on the Axepta payment page by the server, once per payment, with 3-D Secure followed as far as it goes without a person; the payment page is handed out when the card never got there.

### Changed

- **The MCP server reads each pub's menu once a day.** Menus are cached in memory per sales area until midnight in London; quotes are still priced live at the till, and a basket the till refuses refreshes the menu.

## 0.2.0

### Added

- **Ordering API in the client.** Tables, basket validation and the payment API (prepare a card payment, read its status, cancel it), with basket lines built from menu products. Payment requests that could place an order are never retried.
- **`spoons mcp`.** An MCP server for ordering to a table: find a pub, read its menu, quote a basket, pay through the card payment page and follow the order. Payments are never prepared twice for a quote or retried, and nothing cancels while a card may be being authorised.
- **`spoons mcp --http` and a container image.** The MCP server runs over streamable HTTP behind a gateway, taking each user's receipt address from the `X-Spoons-Email` header, and releases publish `ghcr.io/radiosilence/spoons-mcp`.

### Fixed

- **Payments at version 2.0 of the payment API.** The web app moved to it, and preparing a payment at the old routes answered 404. The client now reads the version from the app's bundle as the app does.

### Internal

- **Builds use the latest stable Rust.** mise installs `rust` at `latest`.

## 0.1.1

### Added

- **Releases with binaries.** `just release` opens a release pull request; merging it publishes a GitHub release with the `spoons` binary for Linux (x86_64, arm64) and Apple silicon macOS. Both crates now share the workspace version.

### Security

- **Dependencies updated for RustSec advisories.** Fixes rustls (RUSTSEC-2026-0346/0347/0348/0349), rustls-webpki (RUSTSEC-2026-0382) and webpki-root-certs (RUSTSEC-2026-0383).
- Update tokio and other packages for security

### Internal

- **CI and changelog fragments.** Pull requests are checked for formatting, clippy warnings, tests and changelog fragments, and `CHANGELOG.md` is generated from `changelog.d/` so concurrent pull requests never conflict on it.
- **One CI pipeline gated by `Checks`.** The workflow is now `ci-cd.yml`; its Rust jobs run only when a change touches Rust sources, and `Checks` is the single status the ruleset requires.
- **Workflow lint, dependency audit and dependabot.** CI runs actionlint on every change and a blocking `cargo audit` on dependency changes (every push to main), dependabot raises weekly grouped updates for actions and crates, and a pre-push hook runs `just fast`.
