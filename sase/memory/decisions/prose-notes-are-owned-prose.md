---
keyword: Prose Notes Are Owned Prose
summary: "Prose notes are author-owned writing; legacy type and CLI spellings stay aliases."
metadata:
  status: accepted
---

# Prose Notes Are Owned Prose

**Applies to.** bob-cli, bob-plugins, and the Bob vault.

**Claim.** The note type is named `prose` and is defined by authorship and primary
role, as described by [[glossary:prose-note]]. The legacy `[[article]]` spelling and
`bob articles` command are permanent aliases. Wire vocabulary keeps its legacy names.
“Sections implicitly use the type” means derived eligibility for extraction, never
in-place formatting or a stored section type.

**Why.** The prose name covers authored writing across genres and works for both prose
notes and prose sections. The ownership rule distinguishes them from areas, projects,
and other notes that happen to contain paragraphs. Derived section eligibility keeps
formatting confined to the note type and leaves the enclosing note's role intact.

Evidence: research report
`research:202611/prose_rename_and_section_extraction/prose_rename_and_section_extraction.md`;
bob-cli commit `4168e201` (`feat(prose): make prose the canonical note type and CLI`,
phase `bob-cli-qj.2`).

**Cost.** bob keeps recognizing the legacy type and command, and the JSON contract
retains article-named wire keys and refusal codes. Clients and docs must distinguish
those compatibility names from the canonical type name.

**Reopens when.** The ownership and primary-role rule changes, section eligibility
becomes stored or permits in-place formatting, or a new reviewed decision revises the
permanent alias or wire-compatibility contract.
