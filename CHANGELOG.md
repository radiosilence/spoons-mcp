# Changelog

## 0.5.1

### Fixed

- **Smoother on a phone.** The quote's button is now the acceptance: "Confirm payment" with the total and table, and one tap pays, instead of asking in the chat. `ui` hints are read whether they come as an object or, as models often send them, a JSON string, instead of failing the call; and the `spoons` tool is no longer marked destructive, so hosts stop asking permission before every menu lookup. Paying in the chat still goes through PREVIEW and CONFIRM on the user's say-so.

### Internal

- **The Mac build no longer holds anything up.** It builds in its own job that neither Checks, the image nor the GitHub release waits on, and is attached to the release once built.

## 0.5.0

### Added

- **Product options and meal deals.** `menu` lists each product's option groups (sides, peas, mixers) with ids, labels and prices, and the meal deals it belongs to; `quote` and `basketAdd` take options by id or by what was said ("chips", "mushy peas", "neat"), refuse a missing or unknown choice with the valid ones listed, and add a meal's included drink, with the deal's saving on the quote. The app asks with a sheet of choices and a drink picker.
- **The menu as Markdown.** `pub { menu { markdown } }` gives the whole menu, or the part `kinds` and `search` pick, as Markdown with sizes, prices, guest ales, options and deals; with no `ref` or `table` it's the conversation's pub and table, so "a markdown menu of the pub I'm in" is one query.

### Changed

- **Search finds what was said, not just what's printed.** Pubs and products match misheard words, brand styling ("sumo" finds Mad Squirrel $umo) and what a drink is ("mocktail" finds the 0% cocktails), across name, the pub's own category, description and notes, ranked best first with close matches when nothing fits exactly; the menu lists the pub's own categories and filters by them.
- **The table sticks.** Once a table is given it serves every later order in the conversation: a quote remembers its pub and table, a quote with no table uses them, and the model asks again only when the user says they've moved, orders at another pub, or comes back hours later. "And pay" with an order quotes and pays in one go. Every quote line now carries the till's price; a guest ale's line was coming back without one.

### Internal

- **jiff 1.0.** The cargo group moves to jiff 1.0, whose `to_zoned` is now `to_zoned_datetime`.

## 0.4.0

### Added

- **An MCP App shows pubs, the menu, the basket, quotes and orders beside the chat in hosts that support MCP Apps.** The menu has kind tabs, an instant filter, the guest ales picked out with their colour, strength and notes, and sizes to tap; the basket keeps the table big and editable; it all goes through the same `spoons` GraphQL, keeps in step with the model both ways, and never pays: paying stays in the conversation.

### Changed

- **The MCP server is a GraphQL API behind two tools, `spoons_schema` and `spoons`, replacing the seven.** Menus are whole, sorted into kinds (guest ales, which never say "ale", included) with ABV, units, calories, colour and taste notes; each conversation keeps a basket on the server; and `pay`, `cancelOrder` and `sendDuplicate` are PREVIEW then CONFIRM with a token bound to what was previewed.

## 0.3.3

### Changed

- **Card entry gets through Axepta's 3-D Secure 2 page.** The bank's device check is skipped as the page's script skips it on a timeout, and Axepta's answer is followed: a redirect when no challenge is needed, or a clear error when the bank wants the card holder to approve.

## 0.3.2

### Changed

- **Card entry logs the page it stops on.** When 3-D Secure reaches a page that doesn't submit itself, the server logs an outline of it (forms and field names, frames, scripts) for working out the next step.

## 0.3.1

### Changed

- **The MCP server's instructions cover repeat orders.** The assistant takes the pub, table and last order from the conversation for "same again", and asks for a table rather than guessing one.

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
