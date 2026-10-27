# Vendored Defuddle (web-clip article extraction)

- Version: 0.19.4 (npm package `defuddle@0.19.4`)
- Source file in the package: `package/dist/index.full.js` (UMD, sets
  `window.Defuddle`), fetched with `npm pack defuddle@0.19.4`
- Vendored as: `defuddle.full.js`
- SHA-256: `af3405fbab6971f85ad9912f7b4dba4d0889900efe5d93638df7d3f23278ebc1`
- License: MIT (`DEFUDDLE_LICENSE`, copied from the package `LICENSE`);
  copyright Steph Ango (@kepano)

The adapter injects this bundle into an isolated offline page with
`page.add_script_tag(content=...)` and runs
`new Defuddle(document, { url }).parse()`.

## Mermaid (diagram rendering)

- Version: 12.2.0 (`mermaid@12.2.0`)
- Source file in the package: `dist/mermaid.min.js` (self-contained IIFE,
  sets `globalThis.mermaid`), fetched with `npm pack mermaid@12.2.0`
- Vendored as: `mermaid.min.js`
- SHA-256: `a8a6a6f805b074a8261a2825c86f99c8054acb3b35f50bb7e24a4f9f2c3c3b9f`
- License: MIT (`MERMAID_LICENSE`, copied from the package `LICENSE`)
- The bundle contains no `import(` calls or lazy chunks.

The adapter injects this bundle into one offline Playwright page per document and
uses it to render each Mermaid block to a fitted vector PDF.
