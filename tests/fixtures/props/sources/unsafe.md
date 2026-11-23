---
defaults: &base {x: 1}
copy: *base
tagged: !custom 1
merge:
  <<: {a: 1}
  b: 2
quoted: "looks like *alias and !tag"
block: |
  *not_an_alias
created: 2026-11-22
---
body
