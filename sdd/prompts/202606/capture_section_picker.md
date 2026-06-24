---
plan: sdd/tales/202606/capture_section_picker.md
---
 When I use the `<cmd+ctrl+shift+i>` keymap (configured via Hammerspoon in my chezmoi repo) and the user
input ends in `@` or `@#`, the user is prompted to pick an area or project note file. Can you help me add a similar
functionality for the `#` symbol? Namely, when no characters are provided after `#`, we should prompt the user to select
one of the sections in the selected file. This prompt should be after the project/area note file prompt (if one was
necessary) and we should only show this prompt when there are multiple non-"Tasks" sections to choose from (otherwise,
we should just add the new bullet to the only non-"Tasks" section in the selected area / project note file). Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the
`sase plan propose` command (as the skill instructs) before making any file changes.



%xprompts_enabled:false
### Questions and Answers

#### Q1: Trigger scope

> When should the new section picker appear? Today, a trailing bare `@#` opens the note picker and then silently drops the bullet into the first non-Tasks section, while an explicit trailing route like `@cash#` is passed straight to bob (also first non-Tasks section, no picker). Which trailing forms should trigger the section picker (only when 2+ non-Tasks sections exist)?

- [ ] **Bare @# only** — Only a trailing @# triggers it: pick the note, then pick a section. Explicit @cash# keeps current silent behavior. Smallest change, matches the literal request.
- [x] **Both @# and @route#** — Also intercept a trailing explicit @cash#: skip the note picker (note already known), go straight to the section picker. Matches the after-the-note-prompt-if-necessary wording; changes current @cash# behavior.

#### Q2: Prefix form

> For the @#prefix form (a section prefix IS typed, e.g. ... @#Idea), what should happen?

- [ ] **Keep current behavior** — A typed prefix means the section is already chosen: keep synthesizing @route#Idea and let bob prefix-match. Never show the section picker when a prefix is present.
- [ ] **Picker if prefix ambiguous** — If the typed prefix matches 2+ non-Tasks sections, show the section picker pre-filtered to those; otherwise use the single match. More work, more surprising.

#### Q3: Section list

> Which non-Tasks headings should the section picker list?

- [ ] **All non-Tasks headings (any level)** — List every ATX heading whose title is not Tasks, H1-H6, in document order. Matches the set bob can already target.
- [ ] **Top-level only (H1/H2)** — List only top-level section headings, hiding deeply nested sub-headings from the picker.

---

> **Global Note:** Answered via Telegram

%xprompts_enabled:true