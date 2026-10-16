-- Bob note snapshot print profile (see docs/highlights-create.md).
--
-- Third --lua-filter after code-break/listen and return-links. It maps the
-- projected-Markdown contract to note.tex macros by wrapping AST content
-- with fixed RawInline/RawBlock delimiters. It never stringifies user
-- content into a TeX string with pandoc.write. Links, emphasis, code,
-- anchors, and return-link additions remain AST content.
--
-- Processing order inside Pandoc(doc), using only APIs stable in pandoc
-- 3.1.11.1 and newer:
--   1. read flags and the glyph table (bob-glyph-table),
--   2. strip U+FE0F everywhere (body and metadata),
--   3. cmap-driven glyph wrapping for Str (serif) and Code (mono),
--      selecting another body face before Noto,
--   4. task lists (whitelisted st-* only), spans, links with chips,
--      divs, headers (lane/appendix, safe nav labels, needspace),
--   5. long-token break opportunities for Str longer than 24 chars,
--   6. masthead from metadata AST and H1 navigation from headers.
-- Navigation is built after return-link pairing so its links gain no pills.
-- The existing return-link report file is never touched, preserving the
-- bob-return-links:false opt-out.
local STATUS_MAP = {
  ["st-open"] = "Open",
  ["st-next"] = "Next",
  ["st-wip"] = "Wip",
  ["st-blocked"] = "Blocked",
  ["st-done"] = "Done",
  ["st-canceled"] = "Canceled",
}
local STATUSES = {
  open = true, next = true, wip = true, blocked = true, scheduled = true
}
local STAT_GLYPH = {
  open = "\\BobStOpen{}",
  next = "\\BobStNext{}",
  wip = "\\BobStWip{}",
  blocked = "\\BobStBlocked{}",
  scheduled = "\\BobStBlocked{}",
}
local FE0F = "\239\184\143"
local GLYPHS = { fallback = {}, serif_uncovered = {}, sans_uncovered = {}, mono_uncovered = {} }
local function load_glyph_table(path)
  if not path or path == "" then return end
  local ok, tbl = pcall(dofile, path)
  if not ok or type(tbl) ~= "table" then return end
  for k, v in pairs(tbl) do
    if type(k) == "number" and v then GLYPHS.fallback[k] = true end
  end
  for _, name in ipairs({ "serif_uncovered", "sans_uncovered", "mono_uncovered" }) do
    local sub = tbl[name]
    if type(sub) == "table" then
      for k, v in pairs(sub) do
        if type(k) == "number" and v then GLYPHS[name][k] = true end
      end
    end
  end
end
local function strip_fe0f_text(text)
  return (text:gsub(FE0F, ""))
end
local function utf8_codes(text)
  local out = {}
  if utf8 and utf8.codes then
    for _, cp in utf8.codes(text) do out[#out + 1] = cp end
  else
    for i = 1, #text do out[#out + 1] = text:byte(i) end
  end
  return out
end
local function utf8_char(cp)
  if utf8 and utf8.char then
    local ok, ch = pcall(utf8.char, cp)
    if ok then return ch end
  end
  return string.char(cp % 256)
end
local function wrap_kind_for_str(cp)
  if GLYPHS.fallback[cp] then return "fallback" end
  if GLYPHS.serif_uncovered[cp] then
    if not GLYPHS.mono_uncovered[cp] then return "mono" end
    if not GLYPHS.sans_uncovered[cp] then return "sans" end
    return "fallback"
  end
  return nil
end
local function wrap_kind_for_code(cp)
  if GLYPHS.fallback[cp] then return "fallback" end
  if GLYPHS.mono_uncovered[cp] then
    if not GLYPHS.serif_uncovered[cp] then return "serif" end
    if not GLYPHS.sans_uncovered[cp] then return "sans" end
    return nil
  end
  return nil
end
local BEGIN_END = {
  fallback = { "\\BobFallbackBegin{}", "\\BobFallbackEnd{}" },
  mono = { "\\BobMonoGlyphBegin{}", "\\BobMonoGlyphEnd{}" },
  sans = { "\\BobSansGlyphBegin{}", "\\BobSansGlyphEnd{}" },
  serif = { "\\BobSerifGlyphBegin{}", "\\BobSerifGlyphEnd{}" },
}
local function raw_latex(text) return pandoc.RawInline("latex", text) end
local function raw_block(text) return pandoc.RawBlock("latex", text) end
-- Split Str text into AST inlines with glyph wrapping and long-token breaks.
-- Long Str (>24 chars) gains \allowbreak{} after _, /, -, and . with no
-- visible spacers. Never stringifies surrounding content.
local function wrap_str_text(text)
  text = strip_fe0f_text(text)
  local cps = utf8_codes(text)
  if #cps == 0 then return { pandoc.Str("") } end
  local out = {}
  local buf = {}
  local cur_kind = nil
  local function flush_buf()
    if #buf == 0 then return end
    local chunk = table.concat(buf)
    buf = {}
    if cur_kind == nil then
      if #utf8_codes(chunk) > 24 then
        local parts = {}
        local piece = {}
        for _, cp in ipairs(utf8_codes(chunk)) do
          piece[#piece + 1] = utf8_char(cp)
          if cp == 0x5F or cp == 0x2F or cp == 0x2D or cp == 0x2E then
            parts[#parts + 1] = table.concat(piece)
            parts[#parts + 1] = "\0BREAK\0"
            piece = {}
          end
        end
        if #piece > 0 then parts[#parts + 1] = table.concat(piece) end
        for _, part in ipairs(parts) do
          if part == "\0BREAK\0" then
            out[#out + 1] = raw_latex("\\allowbreak{}")
          elseif part ~= "" then
            out[#out + 1] = pandoc.Str(part)
          end
        end
      else
        out[#out + 1] = pandoc.Str(chunk)
      end
    else
      local be = BEGIN_END[cur_kind]
      out[#out + 1] = raw_latex(be[1])
      out[#out + 1] = pandoc.Str(chunk)
      out[#out + 1] = raw_latex(be[2])
    end
    cur_kind = nil
  end
  for _, cp in ipairs(cps) do
    local kind = wrap_kind_for_str(cp)
    if kind ~= cur_kind then flush_buf() cur_kind = kind end
    buf[#buf + 1] = utf8_char(cp)
  end
  flush_buf()
  return out
end
-- Split Code text with mono-role fallback wrapping (no long-token breaks;
-- the code-break filter already ran).
local function wrap_code_text(text)
  text = strip_fe0f_text(text)
  local cps = utf8_codes(text)
  local out = {}
  local buf = {}
  local cur_kind = nil
  local function flush_buf(is_code)
    if #buf == 0 then return end
    local chunk = table.concat(buf)
    buf = {}
    if cur_kind == nil then
      out[#out + 1] = pandoc.Code(chunk)
    else
      local be = BEGIN_END[cur_kind]
      out[#out + 1] = raw_latex(be[1])
      out[#out + 1] = pandoc.Str(chunk)
      out[#out + 1] = raw_latex(be[2])
    end
  end
  for _, cp in ipairs(cps) do
    local kind = wrap_kind_for_code(cp)
    if kind ~= cur_kind then flush_buf() cur_kind = kind end
    buf[#buf + 1] = utf8_char(cp)
  end
  flush_buf()
  return out
end
-- Parse a textual div/meta attribute as AST inlines without stringifying
-- surrounding content. Falls back to a plain Str on parse failure.
local function parse_inlines(text)
  if not text or text == "" then return {} end
  local ok, doc = pcall(pandoc.read, text, "commonmark_x")
  if ok and doc and doc.blocks and #doc.blocks > 0 then
    local first = doc.blocks[1]
    if first.t == "Para" or first.t == "Plain" then
      return first.content
    end
  end
  return { pandoc.Str(text) }
end
local function is_inlines_list(value)
  if type(value) ~= "table" then return false end
  if value.t ~= nil then return false end
  if #value == 0 then return false end
  local first = value[1]
  -- Inline elements are userdata in pandoc's Lua, not tables.
  local ok, tag = pcall(function() return first.t end)
  return ok and tag ~= nil
end
local function meta_to_inlines(value)
  if value == nil then return nil end
  if type(value) == "string" then return parse_inlines(value) end
  if type(value) ~= "table" then return nil end
  if value.t == "MetaInlines" then return value.content end
  if value.t == "MetaString" then return parse_inlines(value.text) end
  if is_inlines_list(value) then return value end
  return nil
end
local function meta_to_string(value)
  if value == nil then return nil end
  if type(value) == "string" then return value end
  if type(value) == "boolean" or type(value) == "number" then return tostring(value) end
  if type(value) ~= "table" then return nil end
  local ok, s = pcall(pandoc.utils.stringify, value)
  if ok then return s end
  return nil
end
local function is_meta_list(value)
  if value == nil or type(value) ~= "table" then return false end
  if value.t == "MetaList" then return true end
  return value.t == nil and #value > 0
end
local function is_meta_map(value)
  if value == nil or type(value) ~= "table" then return false end
  if value.t == "MetaMap" then return true end
  return value.t == nil and (value["status"] ~= nil or value["count"] ~= nil)
end
-- soul's strikeout reconstruction cannot parse raw font wrappers or inline
-- code. Keep ordinary words visibly struck, and leave those already
-- formatted runs intact so one glyph or code span cannot abort the PDF.
local function strikeout_with_plain_runs(content)
  local out = {}
  local run = {}
  local outside_font_wrapper = false
  local function flush_run()
    if #run > 0 then
      out[#out + 1] = pandoc.Strikeout(run)
      run = {}
    end
  end
  for _, inline in ipairs(content) do
    local raw = inline.t == "RawInline" and inline.format == "latex" and inline.text or ""
    if raw:match("^\\Bob[%a]+Begin{}$") then
      flush_run()
      outside_font_wrapper = true
      out[#out + 1] = inline
    elseif outside_font_wrapper then
      out[#out + 1] = inline
      if raw:match("^\\Bob[%a]+End{}$") then
        outside_font_wrapper = false
      end
    elseif inline.t == "Str" or inline.t == "Space" or inline.t == "SoftBreak" or inline.t == "LineBreak" then
      run[#run + 1] = inline
    else
      flush_run()
      out[#out + 1] = inline
    end
  end
  flush_run()
  return out
end
-- Recursively process inlines: Str/Code glyph wrapping, Span/Link mapping.
-- Returns a new inline list, preserving anchors and return-link additions.
local function process_inlines(inlines)
  local out = {}
  for _, el in ipairs(inlines) do
    if el.t == "Str" then
      for _, n in ipairs(wrap_str_text(el.text)) do out[#out + 1] = n end
    elseif el.t == "Code" then
      for _, n in ipairs(wrap_code_text(el.text)) do
        if n.t == "Code" then
          n.attr = el.attr
          out[#out + 1] = n
        else
          out[#out + 1] = n
        end
      end
    elseif el.t == "Span" then
      local classes = {}
      if el.classes then for _, c in ipairs(el.classes) do classes[c] = true end end
      local content = process_inlines(el.content)
      if classes["chip"] and classes["chip-accent"] then
        out[#out + 1] = raw_latex("\\BobChipAccentBegin{}")
        for _, n in ipairs(content) do out[#out + 1] = n end
        out[#out + 1] = raw_latex("\\BobChipAccentEnd{}")
      elseif classes["chip"] then
        out[#out + 1] = raw_latex("\\BobChipBegin{}")
        for _, n in ipairs(content) do out[#out + 1] = n end
        out[#out + 1] = raw_latex("\\BobChipEnd{}")
      elseif classes["label"] then
        out[#out + 1] = raw_latex("\\BobLabelBegin{}")
        for _, n in ipairs(content) do out[#out + 1] = n end
        out[#out + 1] = raw_latex("\\BobLabelEnd{}")
      elseif classes["muted"] then
        out[#out + 1] = raw_latex("\\BobMutedBegin{}")
        for _, n in ipairs(content) do out[#out + 1] = n end
        out[#out + 1] = raw_latex("\\BobMutedEnd{}")
      elseif classes["missing"] then
        out[#out + 1] = raw_latex("\\BobMissingBegin{}")
        for _, n in ipairs(content) do out[#out + 1] = n end
        out[#out + 1] = raw_latex("\\BobMissingEnd{}")
      elseif classes["st"] then
        for _, n in ipairs(content) do out[#out + 1] = n end
      else
        el.content = content
        out[#out + 1] = el
      end
    elseif el.t == "Link" then
      local classes = {}
      if el.classes then for _, c in ipairs(el.classes) do classes[c] = true end end
      el.content = process_inlines(el.content)
      if classes["chip"] and classes["chip-accent"] then
        out[#out + 1] = raw_latex("\\BobChipAccentBegin{}")
        out[#out + 1] = el
        out[#out + 1] = raw_latex("\\BobChipAccentEnd{}")
      elseif classes["chip"] then
        out[#out + 1] = raw_latex("\\BobChipBegin{}")
        out[#out + 1] = el
        out[#out + 1] = raw_latex("\\BobChipEnd{}")
      else
        out[#out + 1] = el
      end
    elseif el.t == "Strikeout" then
      for _, n in ipairs(strikeout_with_plain_runs(process_inlines(el.content))) do
        out[#out + 1] = n
      end
    elseif el.t == "Emph" or el.t == "Strong" or el.t == "Superscript" or el.t == "Subscript" or el.t == "SmallCaps" then
      el.content = process_inlines(el.content)
      out[#out + 1] = el
    elseif el.t == "Quoted" then
      el.content = process_inlines(el.content)
      out[#out + 1] = el
    elseif el.t == "Cite" then
      el.content = process_inlines(el.content)
      out[#out + 1] = el
    elseif el.t == "Image" then
      if el.caption then el.caption = process_inlines(el.caption) end
      out[#out + 1] = el
    elseif el.t == "Note" then
      el.content = process_blocks(el.content)
      out[#out + 1] = el
    else
      out[#out + 1] = el
    end
  end
  return out
end
-- Status marker at the start of a task list item: empty Span with .st and
-- one whitelisted .st-* class. Returns the Bob status name or nil.
local function task_status_of_item(first_block)
  if not first_block then return nil end
  if first_block.t ~= "Para" and first_block.t ~= "Plain" then return nil end
  local first = first_block.content[1]
  if not first or first.t ~= "Span" then return nil end
  local has_st = false
  local status = nil
  if first.classes then
    for _, c in ipairs(first.classes) do
      if c == "st" then has_st = true end
      if STATUS_MAP[c] then status = STATUS_MAP[c] end
    end
  end
  if has_st and status then return status end
  return nil
end
-- Convert one BulletList to BobTasks with AST-preserving wrappers.
-- Only status-marked items become BobTask rows; other items keep muted
-- bullets via BobOrdinaryItem. Every child block is preserved, including
-- nested lists (already processed bottom-up by explicit recursion).
local function convert_task_list(items)
  local out = {}
  out[#out + 1] = raw_block("\\begin{BobTasks}")
  for _, item in ipairs(items) do
    -- Detect the status on the unprocessed item: process_inlines drops the
    -- empty .st marker, so re-detecting after processing would miss it.
    local status = task_status_of_item(item[1])
    local blocks = process_blocks(item)
    if status then
      -- The marker Span is already consumed by process_inlines (empty .st
      -- emits nothing); drop a leading empty Para/Plain if one remains and
      -- keep anchors, links, chips, and child blocks.
      if blocks[1] and (blocks[1].t == "Para" or blocks[1].t == "Plain") and #blocks[1].content == 0 then
        table.remove(blocks, 1)
      end
      out[#out + 1] = raw_block("\\BobTask{" .. status .. "}")
      for _, b in ipairs(blocks) do out[#out + 1] = b end
    else
      out[#out + 1] = raw_block("\\BobOrdinaryItem{}")
      for _, b in ipairs(blocks) do out[#out + 1] = b end
    end
  end
  out[#out + 1] = raw_block("\\end{BobTasks}")
  return out
end
-- Recursively process blocks bottom-up (children before parents).
function process_blocks(blocks)
  local out = {}
  for _, blk in ipairs(blocks) do
    if blk.t == "Para" or blk.t == "Plain" then
      blk.content = process_inlines(blk.content)
      -- Center standalone images (a Para with a single Image) with
      -- environment delimiters; the Image itself stays AST.
      if blk.t == "Para" and #blk.content == 1 and blk.content[1].t == "Image" then
        out[#out + 1] = raw_block("\\begin{center}")
        out[#out + 1] = blk
        out[#out + 1] = raw_block("\\end{center}")
      else
        out[#out + 1] = blk
      end
    elseif blk.t == "Header" then
      blk.content = process_inlines(blk.content)
      local has_lane = false
      if blk.classes then
        for _, c in ipairs(blk.classes) do if c == "lane" then has_lane = true end end
      end
      out[#out + 1] = raw_block("\\needspace{4\\baselineskip}")
      if has_lane then
        out[#out + 1] = pandoc.Para({ raw_latex("\\BobLaneBegin{}") })
        local para_inlines = {}
        for _, n in ipairs(blk.content) do para_inlines[#para_inlines + 1] = n end
        para_inlines[#para_inlines + 1] = raw_latex("\\BobLaneEnd{}")
        out[#out + 1] = pandoc.Para(para_inlines)
      else
        out[#out + 1] = blk
      end
    elseif blk.t == "BulletList" then
      local has_task = false
      for _, item in ipairs(blk.content) do
        if task_status_of_item(item[1]) then has_task = true break end
      end
      if has_task then
        for _, b in ipairs(convert_task_list(blk.content)) do out[#out + 1] = b end
      else
        local items = {}
        for _, item in ipairs(blk.content) do items[#items + 1] = process_blocks(item) end
        blk.content = items
        out[#out + 1] = blk
      end
    elseif blk.t == "OrderedList" then
      local done = {}
      for _, item in ipairs(blk.content) do done[#done + 1] = process_blocks(item) end
      blk.content = done
      out[#out + 1] = blk
    elseif blk.t == "Div" then
      local classes = {}
      if blk.classes then for _, c in ipairs(blk.classes) do classes[c] = true end end
      local content = process_blocks(blk.content)
      if classes["embed"] then
        local source_text = nil
        if blk.attributes and blk.attributes["source"] then source_text = blk.attributes["source"] end
        out[#out + 1] = raw_block("\\begin{BobEmbed}")
        if source_text then
          local src = process_inlines(parse_inlines(source_text))
          local para = { raw_latex("\\BobEmbedSourceBegin{}") }
          for _, n in ipairs(src) do para[#para + 1] = n end
          para[#para + 1] = raw_latex("\\BobEmbedSourceEnd{}")
          out[#out + 1] = pandoc.Para(para)
        end
        for _, b in ipairs(content) do out[#out + 1] = b end
        out[#out + 1] = raw_block("\\end{BobEmbed}")
      elseif classes["livequery"] then
        out[#out + 1] = raw_block("\\begin{BobLiveQuery}")
        for _, b in ipairs(content) do out[#out + 1] = b end
        out[#out + 1] = raw_block("\\end{BobLiveQuery}")
      elseif classes["callout"] then
        local kind = "note"
        if blk.attributes and blk.attributes["kind"] then
          local raw_kind = blk.attributes["kind"]
          if raw_kind:match("^[A-Za-z0-9_-]+$") then kind = raw_kind end
        end
        out[#out + 1] = raw_block("\\begin{BobCallout}{" .. kind .. "}")
        if blk.attributes and blk.attributes["title"] and blk.attributes["title"] ~= "" then
          local title_inlines = process_inlines(parse_inlines(blk.attributes["title"]))
          local para = { raw_latex("\\BobCalloutTitleBegin{}") }
          for _, n in ipairs(title_inlines) do para[#para + 1] = n end
          para[#para + 1] = raw_latex("\\BobCalloutTitleEnd{}")
          out[#out + 1] = pandoc.Para(para)
        end
        for _, b in ipairs(content) do out[#out + 1] = b end
        out[#out + 1] = raw_block("\\end{BobCallout}")
      elseif classes["compact"] then
        out[#out + 1] = raw_block("\\begin{BobCompact}")
        for _, b in ipairs(content) do out[#out + 1] = b end
        out[#out + 1] = raw_block("\\end{BobCompact}")
      else
        blk.content = content
        out[#out + 1] = blk
      end
    elseif blk.t == "CodeBlock" then
      blk.text = strip_fe0f_text(blk.text)
      out[#out + 1] = blk
    elseif blk.t == "RawBlock" then
      out[#out + 1] = blk
    elseif blk.t == "Table" or blk.t == "Figure" then
      out[#out + 1] = blk
    else
      if blk.content and type(blk.content) == "table" then
        local ok, _ = pcall(function() blk.content = process_inlines(blk.content) end)
        if not ok then end
      end
      out[#out + 1] = blk
    end
  end
  return out
end
local function para_wrapped(begin_tex, inlines, end_tex)
  local out = { raw_latex(begin_tex) }
  for _, n in ipairs(inlines) do out[#out + 1] = n end
  out[#out + 1] = raw_latex(end_tex)
  return pandoc.Para(out)
end
function Pandoc(doc)
  if not FORMAT:match("latex") then return nil end
  local glyph_path = meta_to_string(doc.meta["bob-glyph-table"])
  if glyph_path and glyph_path ~= "" then
    load_glyph_table(glyph_path)
  else
    load_glyph_table("glyphs.lua")
  end
  -- Strip FE0F from metadata strings up front (body Str/Code handled in
  -- process_inlines/process_blocks).
  for k, v in pairs(doc.meta) do
    if type(v) == "table" and v.t == "MetaString" and type(v.text) == "string" then
      v.text = strip_fe0f_text(v.text)
    end
  end
  doc.blocks = process_blocks(doc.blocks)
  -- Collect H1 headers for navigation; place safe TeX labels immediately
  -- after each H1 (not inside the title, which would break LaTeX's label
  -- hooks via texorpdfstring) for page refs, rather than interpolating
  -- arbitrary heading ids into raw TeX.
  local h1s = {}
  local safes = {}
  for _, blk in ipairs(doc.blocks) do
    if blk.t == "Header" and blk.level == 1 then h1s[#h1s + 1] = blk end
  end
  local with_labels = {}
  local h1_index = 0
  for _, blk in ipairs(doc.blocks) do
    with_labels[#with_labels + 1] = blk
    if blk.t == "Header" and blk.level == 1 then
      h1_index = h1_index + 1
      local safe = "bobsec" .. tostring(h1_index)
      safes[h1_index] = safe
      with_labels[#with_labels + 1] = raw_block("\\label{" .. safe .. "}")
    end
  end
  doc.blocks = with_labels
  -- Build the masthead from the metadata AST (never stringified).
  local mast = {}
  mast[#mast + 1] = raw_block("\\BobMastheadBegin{}")
  local kicker = meta_to_inlines(doc.meta["bob-kicker"])
  if kicker then mast[#mast + 1] = para_wrapped("\\BobKickerBegin{}", process_inlines(kicker), "\\BobKickerEnd{}") end
  local display = meta_to_inlines(doc.meta["bob-display-title"])
  if display then mast[#mast + 1] = para_wrapped("\\BobDisplayTitleBegin{}", process_inlines(display), "\\BobDisplayTitleEnd{}") end
  local dek = meta_to_inlines(doc.meta["bob-dek"])
  if dek then mast[#mast + 1] = para_wrapped("\\BobDekBegin{}", process_inlines(dek), "\\BobDekEnd{}") end
  local meta_list = doc.meta["bob-meta"]
  if is_meta_list(meta_list) then
    local items = meta_list.content or meta_list
    local joined = {}
    for idx, item in ipairs(items) do
      local inlines = meta_to_inlines(item)
      if inlines then
        if idx > 1 then
          joined[#joined + 1] = pandoc.Space()
          joined[#joined + 1] = pandoc.Str("·")
          joined[#joined + 1] = pandoc.Space()
        end
        for _, n in ipairs(process_inlines(inlines)) do joined[#joined + 1] = n end
      end
    end
    if #joined > 0 then mast[#mast + 1] = para_wrapped("\\BobMetaBegin{}", joined, "\\BobMetaEnd{}") end
  end
  local stats = doc.meta["bob-stats"]
  if is_meta_list(stats) then
    local items = stats.content or stats
    local joined = {}
    for _, item in ipairs(items) do
      if is_meta_map(item) then
        local fields = item.content or item
        local st_raw = meta_to_string(fields["status"])
        local count_raw = meta_to_string(fields["count"])
        if st_raw and STATUSES[st_raw] and count_raw and tonumber(count_raw) then
          if #joined > 0 then
            joined[#joined + 1] = pandoc.Space()
            joined[#joined + 1] = pandoc.Str("·")
            joined[#joined + 1] = pandoc.Space()
          end
          joined[#joined + 1] = raw_latex(STAT_GLYPH[st_raw])
          joined[#joined + 1] = pandoc.Space()
          joined[#joined + 1] = pandoc.Str(tostring(tonumber(count_raw)))
          joined[#joined + 1] = pandoc.Space()
          joined[#joined + 1] = pandoc.Str(st_raw)
        end
      end
    end
    local omitted = meta_to_inlines(doc.meta["bob-omitted"])
    if omitted then
      if #joined > 0 then
        joined[#joined + 1] = pandoc.Space()
        joined[#joined + 1] = pandoc.Str("·")
        joined[#joined + 1] = pandoc.Space()
      end
      for _, n in ipairs(process_inlines(omitted)) do joined[#joined + 1] = n end
    end
    if #joined > 0 then mast[#mast + 1] = para_wrapped("\\BobStatsBegin{}", joined, "\\BobStatsEnd{}") end
  end
  if #h1s > 0 then
    local nav = { raw_latex("\\BobNavBegin{}") }
    for i, h in ipairs(h1s) do
      if i > 1 then nav[#nav + 1] = raw_latex("\\BobNavSep{}") end
      -- Clone heading inlines including glyph-fallback wrappers (safe labels
      -- now live after headings, not inside them, so nothing is filtered).
      local link_inlines = {}
      for _, n in ipairs(h.content) do link_inlines[#link_inlines + 1] = n end
      if #link_inlines == 0 then link_inlines = { pandoc.Str("Section") } end
      nav[#nav + 1] = pandoc.Link(link_inlines, "#" .. h.identifier)
      nav[#nav + 1] = raw_latex("\\BobNavPage{" .. safes[i] .. "}")
    end
    nav[#nav + 1] = raw_latex("\\BobNavEnd{}")
    mast[#mast + 1] = pandoc.Para(nav)
  end
  local snap = meta_to_inlines(doc.meta["bob-snapshot-line"])
  if snap then mast[#mast + 1] = para_wrapped("\\BobSnapshotLineBegin{}", process_inlines(snap), "\\BobSnapshotLineEnd{}") end
  mast[#mast + 1] = raw_block("\\BobMastheadRule{}")
  mast[#mast + 1] = raw_block("\\BobMastheadEnd{}")
  local new_blocks = {}
  for _, b in ipairs(mast) do new_blocks[#new_blocks + 1] = b end
  for _, b in ipairs(doc.blocks) do new_blocks[#new_blocks + 1] = b end
  doc.blocks = new_blocks
  return doc
end
return { { Pandoc = Pandoc } }
