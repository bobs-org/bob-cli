---
plan: sdd/tales/202606/highlights_ref_contract_1.md
---
 Can you help me make some improvements to the new `bob highlights-ref` command?

- We should start adding the `type` frontmatter property that always has the value of `[[ref]]` to the `~/bob/ref/*.md` note file that we create.
- We should start requiring that the marker note on the PDF file have two fields: `status` (already required) and `parent` (which will be used for the `parent` frontmatter property--so using `[[obsidian]]` by default is no longer necessary).
- We should start using `status: wip` instead of `status: reading` to indicate that the user has started (but not finished) reading the given PDF.

Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the `sase plan`
command (as the skill instructs) before making any file changes.
