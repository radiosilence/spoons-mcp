# Changelog

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
