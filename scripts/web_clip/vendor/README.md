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
