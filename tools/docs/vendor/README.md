# Vendored axe-core browser bundle

- Version: 4.14.0
- Source: https://unpkg.com/axe-core@4.14.0/axe.min.js (official npm CDN for the `axe-core` package; release tag `v4.14.0` at https://github.com/dequelabs/axe-core)
- License: Mozilla Public License 2.0, see `LICENSE.axe`
- Checksum (SHA-256): see `axe.min.js.sha256`

The audit runner injects this file locally into a Playwright browser context.
It is never served as a public site script, and QA must not download an
unpinned CDN script at runtime.
