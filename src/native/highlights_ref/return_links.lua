-- bob ref create: paired return links (see docs/highlights-create.md).
--
-- Second `--lua-filter` after the code-break/listen filter. It resolves every
-- eligible same-document link, tags it with a sequential modifier-letter tag,
-- and pairs it with a `↩ p. N` return pill row at its target. Links that
-- cannot resolve become plain text. It writes one JSON report for bob to the
-- path passed via `-M bob-return-links-report=<path>` and writes nothing to
-- stderr.

local MOD = { a="ᵃ", b="ᵇ", c="ᶜ", d="ᵈ", e="ᵉ", f="ᶠ", g="ᵍ", h="ʰ", i="ⁱ", j="ʲ",
  k="ᵏ", m="ᵐ", n="ⁿ", p="ᵖ", r="ʳ", s="ˢ", t="ᵗ", u="ᵘ", v="ᵛ", w="ʷ", x="ˣ", y="ʸ", z="ᶻ" }
local ALPHABET = "abcdefghijkmnprstuvwxyz" -- no l (reads as 1), o (degree sign), q (no glyph)
local N = #ALPHABET

local function letters(n)
  local out = ""
  while n > 0 do
    local r = (n - 1) % N
    out = ALPHABET:sub(r + 1, r + 1) .. out
    n = (n - 1 - r) // N
  end
  return out
end

local function glyphs(tag) return (tag:gsub("%a", MOD)) end

-- GitHub-style alias via pandoc's own gfm reader: render the heading through
-- gfm and read back the identifier it was assigned. A write-then-read round
-- trip (not `pandoc.utils.stringify`) so code spans keep their markup:
-- `` `__init__` method `` yields `__init__-method` like GitHub does.
local function gfm_slug(inlines)
  local ok, written = pcall(pandoc.write, pandoc.Pandoc({ pandoc.Header(1, inlines) }), "gfm")
  if not ok or type(written) ~= "string" then return "" end
  local ok2, doc = pcall(pandoc.read, written, "gfm")
  if not ok2 or not doc or not doc.blocks or not doc.blocks[1] then return "" end
  return doc.blocks[1].identifier or ""
end

local function is_nav_list(list)
  for _, item in ipairs(list.content) do
    local first = item[1]
    if not first or (first.t ~= "Plain" and first.t ~= "Para") or #first.content ~= 1
      or first.content[1].t ~= "Link" or first.content[1].target:sub(1, 1) ~= "#" then
      return false
    end
    for k = 2, #item do
      if not ((item[k].t == "BulletList" or item[k].t == "OrderedList") and is_nav_list(item[k])) then
        return false
      end
    end
  end
  return #list.content > 0
end

-- Link text for a dead-link record: stringified, whitespace-collapsed, cut to
-- 80 characters with `…` (character-safe for UTF-8).
local function short_text(inlines)
  local s = pandoc.utils.stringify(inlines):gsub("%s+", " "):gsub("^%s+", ""):gsub("%s+$", "")
  local len = utf8.len(s)
  if len and len > 80 then
    s = s:sub(1, utf8.offset(s, 81) - 1) .. "…"
  end
  return s
end

function Pandoc(doc)
  if not FORMAT:match("latex") then return nil end
  local report_path = nil
  local report_meta = doc.meta["bob-return-links-report"]
  if report_meta ~= nil then
    report_path = pandoc.utils.stringify(report_meta)
  end
  local function write_report(obj)
    if report_path and report_path ~= "" then
      local file = io.open(report_path, "w")
      if file then
        file:write(pandoc.json.encode(obj))
        file:close()
      end
    end
  end

  -- Opt-out: frontmatter `bob-return-links: false` leaves the doc unchanged.
  local flag = doc.meta["bob-return-links"]
  if flag ~= nil and (flag == false or pandoc.utils.stringify(flag) == "false") then
    write_report({ version = 1, enabled = false })
    return nil
  end

  -- 1. Index every identifier: definition count plus return capability.
  -- Return-capable targets are a Header, a Div, or a Span outside heading,
  -- caption, and table head/foot contexts. Every other id-bearing element is
  -- resolvable but not capable. A duplicated id is never capable: its links
  -- keep working but stay untagged and get no pill row.
  local id_count, capable = {}, {}
  local function note_def(el, is_capable)
    local id = el.identifier
    if id and id ~= "" then
      id_count[id] = (id_count[id] or 0) + 1
      if id_count[id] == 1 then
        capable[id] = is_capable
      else
        capable[id] = false
      end
    end
  end
  doc:walk({
    Header = function(h) note_def(h, true) end,
    Div = function(d) note_def(d, true) end,
    Span = function(s) note_def(s, true) end,
    CodeBlock = function(c) note_def(c, false) end,
    Table = function(t) note_def(t, false) end,
    Figure = function(f) note_def(f, false) end,
    Code = function(c) note_def(c, false) end,
    Image = function(i) note_def(i, false) end,
    Link = function(l) note_def(l, false) end,
  })
  -- Spans inside headings, captions/image descriptions, or table head/foot
  -- rows lose capability: pills cannot attach there. This scan mirrors the
  -- tagging walker's skip contexts exactly at every nesting depth so id
  -- counting, capability classification and rendering decisions agree.
  -- Definition lists, table body cells, Divs, figures and footnotes stay
  -- eligible; only Spans in skip contexts lose capability. Heading text, TOC
  -- entries and bookmarks never gain pills.
  local scan_skip_blocks, scan_skip_inlines
  scan_skip_inlines = function(inlines, skip)
    for _, il in ipairs(inlines) do
      if il.t == "Span" then
        if skip and il.identifier and il.identifier ~= "" then
          capable[il.identifier] = false
        end
        if il.content ~= nil and type(il.content) == "table" then
          scan_skip_inlines(il.content, skip)
        end
      elseif il.t == "Image" then
        -- Image descriptions are figure captions: never capable.
        scan_skip_inlines(il.caption, true)
      elseif il.t == "Note" then
        -- Footnote bodies render as eligible blocks where the mark sits.
        scan_skip_blocks(il.content, false)
      elseif il.t == "Cite" then
        -- Citation metadata never renders (no citeproc): its ids stay
        -- non-capable, while visible content follows the ambient context.
        for _, citation in ipairs(il.citations) do
          scan_skip_inlines(citation.prefix, true)
          scan_skip_inlines(citation.suffix, true)
        end
        scan_skip_inlines(il.content, skip)
      elseif il.t == "Link" then
        if il.content ~= nil and type(il.content) == "table" then
          scan_skip_inlines(il.content, skip)
        end
      elseif il.content ~= nil and type(il.content) == "table" then
        -- Emph, Strong, Strikeout, Superscript, Subscript, SmallCaps,
        -- Underline, Quoted: recurse, keeping the current skip context.
        scan_skip_inlines(il.content, skip)
      end
    end
  end
  scan_skip_blocks = function(blocks, skip)
    skip = skip or false
    for _, b in ipairs(blocks) do
      if b.t == "Header" then
        scan_skip_inlines(b.content, true)
      elseif b.t == "Table" then
        scan_skip_blocks(b.caption.long, true)
        local function rows(rs, cell_skip)
          for _, row in ipairs(rs) do
            for _, cell in ipairs(row.cells) do scan_skip_blocks(cell.contents, cell_skip) end
          end
        end
        rows(b.head.rows, true)
        rows(b.foot.rows, true)
        for _, body in ipairs(b.bodies) do
          rows(body.head, true)
          -- Body cells stay eligible: still recurse so nested headers,
          -- figures, lists and images inside cells disqualify correctly.
          rows(body.body, skip)
        end
      elseif b.t == "Figure" then
        scan_skip_blocks(b.caption.long, true)
        scan_skip_blocks(b.content, skip)
      elseif b.t == "Div" or b.t == "BlockQuote" then
        scan_skip_blocks(b.content, skip)
      elseif b.t == "BulletList" or b.t == "OrderedList" then
        local nav = (not skip) and is_nav_list(b)
        for _, item in ipairs(b.content) do scan_skip_blocks(item, skip or nav) end
      elseif b.t == "DefinitionList" then
        for _, item in ipairs(b.content) do
          scan_skip_inlines(item[1], skip)
          for _, def in ipairs(item[2]) do scan_skip_blocks(def, skip) end
        end
      elseif b.t == "Para" or b.t == "Plain" then
        scan_skip_inlines(b.content, skip)
      elseif b.t == "LineBlock" then
        for _, line in ipairs(b.content) do
          scan_skip_inlines(line, skip)
        end
      else
        -- CodeBlock, RawBlock, HorizontalRule, Null: no Span definitions.
      end
    end
  end
  scan_skip_blocks(doc.blocks, false)

  -- GitHub aliases for headers in document order, with GitHub's `-1`, `-2`
  -- duplicate numbering. An alias claimed by two different header ids is
  -- ambiguous and never resolves.
  local alias, alias_seen, ambiguous = {}, {}, {}
  doc:walk({ Header = function(h)
    local g = gfm_slug(h.content)
    if g ~= "" then
      local n = alias_seen[g]
      alias_seen[g] = (n or -1) + 1
      if n then g = g .. "-" .. (n + 1) end
      if alias[g] and alias[g] ~= h.identifier then ambiguous[g] = true end
      alias[g] = h.identifier
    end
  end })

  -- Generated anchors use this prefix unless some indexed id starts with it.
  local prefix = "bob:ret:"
  do
    local function collides(p)
      for id in pairs(id_count) do
        if id:sub(1, #p) == p then return true end
      end
      return false
    end
    if collides(prefix) then
      local k = 1
      while collides("bob:ret" .. k .. ":") do k = k + 1 end
      prefix = "bob:ret" .. k .. ":"
    end
  end

  -- 2. Resolution order: exact pandoc id, percent-decoded id, unambiguous
  -- GitHub alias. Real ids always beat aliases. Never fuzz by case or text.
  local function resolve(target)
    local raw = target:sub(2)
    if raw == "" then return nil, "missing" end
    if id_count[raw] then return raw, "exact" end
    local decoded = raw:gsub("%%(%x%x)", function(h) return string.char(tonumber(h, 16)) end)
    if id_count[decoded] then return decoded, "decoded" end
    if alias[decoded] and not ambiguous[decoded] then return alias[decoded], "github" end
    if ambiguous[decoded] then return nil, "ambiguous" end
    return nil, "missing"
  end

  -- 3. Tag in reading order. Pills are raw LaTeX inserted later, so generated
  -- return links are never visible to this pass. Footnote links are numbered
  -- where their note mark sits.
  local inbound, order, order_set = {}, {}, {}
  local resolved_order, resolved_set = {}, {}
  local counter, github_count, untagged = 0, 0, 0
  local dead = {}
  local function mark(skip)
    return function(link)
      if link.target:sub(1, 1) ~= "#" then return nil end
      local id, route = resolve(link.target)
      if not id then
        table.insert(dead, { target = link.target, text = short_text(link.content), reason = route })
        return pandoc.Span(link.content) -- plain text: do not promise a jump
      end
      link.target = "#" .. id
      if route == "github" then github_count = github_count + 1 end
      if not resolved_set[id] then
        resolved_set[id] = true
        table.insert(resolved_order, id)
      end
      if skip or not capable[id] then
        untagged = untagged + 1
        return link
      end
      counter = counter + 1
      local anchor, tag = prefix .. counter, letters(counter)
      if not order_set[id] then order_set[id] = true; table.insert(order, id) end
      if not inbound[id] then inbound[id] = {} end
      table.insert(inbound[id], { anchor = anchor, tag = tag })
      link.content:insert(pandoc.RawInline("latex", "\\BobTag{" .. glyphs(tag) .. "}"))
      return { pandoc.RawInline("latex", "\\BobReturnAnchor{" .. anchor .. "}"), link }
    end
  end
  -- Top-down walker with explicit skip contexts. Pandoc's `:walk` runs
  -- bottom-up, so image descriptions (figure captions) would be tagged before
  -- any parent handler could spare them; walking explicitly keeps every skip
  -- decision in one place.
  local tag_mode, skip_mode = mark(false), mark(true)
  local walk_blocks
  local function walk_inlines(inlines, skip)
    local out = pandoc.Inlines({})
    local function emit(result, original)
      if result == nil then
        out:insert(original)
      elseif type(result) == "table" and result.t == nil then
        for _, el in ipairs(result) do out:insert(el) end
      else
        out:insert(result)
      end
    end
    for _, il in ipairs(inlines) do
      if il.t == "Link" then
        emit((skip and skip_mode or tag_mode)(il), il)
      elseif il.t == "Image" then
        -- Image descriptions are figure captions: never decorated.
        il.caption = walk_inlines(il.caption, true)
        out:insert(il)
      elseif il.t == "Note" then
        -- Footnote links are numbered where their note mark sits.
        il.content = walk_blocks(il.content, false)
        out:insert(il)
      elseif il.t == "Cite" then
        -- Bob runs no citeproc: citation prefix/suffix metadata never
        -- renders, so only the visible content carries links. Walking only
        -- content avoids counting one occurrence twice.
        il.content = walk_inlines(il.content, skip)
        out:insert(il)
      elseif il.content ~= nil and type(il.content) == "table" then
        -- Span, Emph, Strong, Strikeout, Superscript, Subscript, SmallCaps,
        -- Underline, Quoted: recurse, keeping the current skip context.
        il.content = walk_inlines(il.content, skip)
        out:insert(il)
      else
        out:insert(il)
      end
    end
    return out
  end
  local function walk_table(t)
    local function cells(rows, cell_skip)
      for _, row in ipairs(rows) do
        for _, cell in ipairs(row.cells) do
          cell.contents = walk_blocks(cell.contents, cell_skip)
        end
      end
    end
    t.caption.long = walk_blocks(t.caption.long, true)
    cells(t.head.rows, true)
    for _, body in ipairs(t.bodies) do
      cells(body.head, true)
      cells(body.body, false)
    end
    cells(t.foot.rows, true)
    return t
  end
  walk_blocks = function(blocks, skip)
    local out = pandoc.Blocks({})
    for _, b in ipairs(blocks) do
      if b.t == "Header" then
        b.content = walk_inlines(b.content, true)
      elseif b.t == "Table" then
        b = walk_table(b)
      elseif b.t == "Figure" then
        b.caption.long = walk_blocks(b.caption.long, true)
        b.content = walk_blocks(b.content, skip)
      elseif b.t == "Div" or b.t == "BlockQuote" then
        b.content = walk_blocks(b.content, skip)
      elseif b.t == "BulletList" or b.t == "OrderedList" then
        local nav = (not skip) and is_nav_list(b)
        local items = pandoc.List({})
        for _, item in ipairs(b.content) do
          items:insert(walk_blocks(item, skip or nav))
        end
        b.content = items
      elseif b.t == "DefinitionList" then
        for _, item in ipairs(b.content) do
          item[1] = walk_inlines(item[1], skip)
          local defs = pandoc.List({})
          for _, def in ipairs(item[2]) do defs:insert(walk_blocks(def, skip)) end
          item[2] = defs
        end
      elseif b.t == "Para" or b.t == "Plain" then
        b.content = walk_inlines(b.content, skip)
      elseif b.t == "LineBlock" then
        local lines = pandoc.List({})
        for _, line in ipairs(b.content) do
          lines:insert(walk_inlines(line, skip))
        end
        b.content = lines
      else
        -- CodeBlock, RawBlock, HorizontalRule, Null: no links to tag.
      end
      out:insert(b)
    end
    return out
  end
  doc.blocks = walk_blocks(doc.blocks, false)

  -- 4. Attach exactly one row per capable id with inbound links, at its single
  -- defining element: after a Header, first inside a Div, inline after a Span.
  local function block_row(id)
    local parts = {}
    for _, e in ipairs(inbound[id]) do
      table.insert(parts, "\\BobBack{" .. e.anchor .. "}{" .. glyphs(e.tag) .. "}")
    end
    return table.concat(parts, "\\BobBackSep{}")
  end
  local function inline_row(id)
    local parts = {}
    for _, e in ipairs(inbound[id]) do
      table.insert(parts, "\\BobBackCompact{" .. e.anchor .. "}{" .. glyphs(e.tag) .. "}")
    end
    return table.concat(parts, "\\BobBackSep{}")
  end
  doc.blocks = doc.blocks:walk({
    Blocks = function(blocks)
      local out = pandoc.Blocks({})
      for i, b in ipairs(blocks) do
        if b.t == "Header" and b.identifier ~= "" and inbound[b.identifier] then
          local nxt = blocks[i + 1]
          if nxt and nxt.t == "Table" then
            out:insert(pandoc.RawBlock("latex", "\\needspace{16\\baselineskip}"))
          end
          out:insert(b)
          out:insert(pandoc.RawBlock("latex", "\\BobBacklinks{" .. block_row(b.identifier) .. "}"))
        elseif b.t == "Div" and b.identifier and b.identifier ~= "" and inbound[b.identifier] then
          b.content:insert(1, pandoc.RawBlock("latex", "\\BobBacklinks{" .. block_row(b.identifier) .. "}"))
          out:insert(b)
        else
          out:insert(b)
        end
      end
      return out
    end,
    Span = function(s)
      if s.identifier and s.identifier ~= "" and inbound[s.identifier] then
        return { s, pandoc.RawInline("latex", "\\BobBackInline{" .. inline_row(s.identifier) .. "}") }
      end
    end,
  })

  -- 5. Report for bob. Empty lists are omitted (`pandoc.json` encodes an
  -- empty Lua table as `{}`, not `[]`); the Rust reader defaults them.
  local duplicates = {}
  for _, id in ipairs(resolved_order) do
    if id_count[id] and id_count[id] > 1 then
      table.insert(duplicates, { id = id, count = id_count[id] })
    end
  end
  local report = {
    version = 1,
    enabled = true,
    prefix = prefix,
    paired = counter,
    targets = #order,
    untagged = untagged,
    github = github_count,
  }
  if #dead > 0 then report.dead = dead end
  if #duplicates > 0 then report.duplicates = duplicates end
  write_report(report)
  return doc
end
