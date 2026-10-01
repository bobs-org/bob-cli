# Bundled reader fonts (web-clip print template)

Variable `wght` woff2 subsets, fetched with `npm pack`:

- `@fontsource-variable/source-serif-4@5.3.0` (`files/source-serif-4-*.woff2`)
- `@fontsource-variable/inter@5.3.0` (`files/inter-*.woff2`)
- `@fontsource-variable/jetbrains-mono@5.3.0`
  (`files/jetbrains-mono-*.woff2`)

Only the `latin` and `latin-ext` `wght` slices are vendored (normal and
italic for Source Serif 4 and Inter; normal only for JetBrains Mono).
System Noto CJK and emoji fonts cover the rest at print time (see
`../template/reader.css`). Total payload is about 0.5 MB (budget: 1 MB).

License: OFL-1.1 for all three families (`OFL.txt`).

| File | SHA-256 |
| ---- | ------- |
| `source-serif-4-latin-wght-normal.woff2` | `c1df4596be5029233ed2afbb8b2f6ea20784b3fb1aa5d6b5c6519ccd85eb3dfb` |
| `source-serif-4-latin-wght-italic.woff2` | `663e7ef3037a56dce81dfc33f68c1e6445995ffd8887991b3c0b68a7689c9da5` |
| `source-serif-4-latin-ext-wght-normal.woff2` | `41529a5b38008d9ea01e28ec18693a714a3216669ee477d83a5b9db999369625` |
| `source-serif-4-latin-ext-wght-italic.woff2` | `515639854d3566c43860d2005770645c590df8b43a0144c70fe2566c33015ede` |
| `inter-latin-wght-normal.woff2` | `3100e775e8616cd2611beecfa23a4263d7037586789b43f035236a2e6fbd4c62` |
| `inter-latin-wght-italic.woff2` | `7291b5970da2237441273c03b424a504b70b18f09791473fab99687dcc314720` |
| `inter-latin-ext-wght-normal.woff2` | `34b9c504cab7a73e37b746343a449132e56cf7b5481af2cb81dc74dcff25c956` |
| `inter-latin-ext-wght-italic.woff2` | `71254244ccb1ec21e14db3480fef6f6826d1136dc4aca1c7d3ca7ce31294fcce` |
| `jetbrains-mono-latin-wght-normal.woff2` | `18be452724bfdc236c074ca94a249a7f41a86752c7d04ab258ce9ed5651f6a7e` |
