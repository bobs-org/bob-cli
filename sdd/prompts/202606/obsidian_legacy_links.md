---
plan: sdd/tales/202606/obsidian_legacy_links.md
---
 Can you help me migrate the legacy file links found in many of my Obsidian vault note files? We should
convert the lines that start with `# ^ = [[<name>]]` to the frontmatter `parent` property and any line of the form
`# [A-Z0-9] = [[<name>]]` should be added to an unordered list under "Related notes:". Also, clean up any lines that
just contain `#` and anyheaders that seem like they should be notes. For example, in the ~/bob/zorg_alt_tools.md file,
consider the following lines:

```
# Alternative note-taking tools that +zorg should take inspiration from.
#
# ^ = [[zorg]]
#
# A = [[asciidoc]]
# B = [[bullet_journal]]
# D = [[docuwiki]]
# J = [[joplin]]
# L = [[logseq]]
# N = [[neorg]]
# O = [[org_mode]]
# R = [[roam]]
```

This should be reduced to the content shown below and `parent: [[zorg]]` should be added to the frontmatter of the
~/bob/zorg_alt_tools.md file:

```
Alternative note-taking tools that +zorg should take inspiration from.

Related notes:

- [[asciidoc]]
- [[bullet_journal]]
- [[docuwiki]]
- [[joplin]]
- [[logseq]]
- [[neorg]]
- [[org_mode]]
- [[roam]]
```

Make sure to update ALL ~/bob/ files with "# ^ = " lines by running the `rg "# \^ = " ~/bob -l` command after you are
done to verify that it has no output. Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the `sase plan`
command (as the skill instructs) before making any file changes.
