-- Bob meeting dossier print profile.
--
-- Maps dossier Markdown (YAML front matter, a `.meeting-tldr` fenced div,
-- Next steps lists, sections, and an optional `.meeting-transcript` appendix)
-- onto meeting.tex macros. User notes stay AST inlines; Lua never
-- stringifies body text into a TeX argument. Processing uses APIs stable in
-- pandoc 3.1.11.1 and newer.
local FE0F = "\239\184\143"
local GLYPHS = { fallback = {}, serif_uncovered = {}, sans_uncovered = {}, mono_uncovered = {} }
local NEXT_STEP_HEADINGS = {
  ["next steps"] = true,
  ["action items"] = true,
  ["follow-ups"] = true,
  ["follow ups"] = true,
}
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
      out[#out + 1] = pandoc.Str(chunk)
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
local function wrap_code_text(text)
  text = strip_fe0f_text(text)
  local cps = utf8_codes(text)
  local out = {}
  local buf = {}
  local cur_kind = nil
  local function flush_buf()
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
local function meta_to_inlines(value)
  if value == nil then return nil end
  if type(value) == "string" then return parse_inlines(value) end
  if type(value) ~= "table" then return nil end
  if value.t == "MetaInlines" then return value.content end
  if value.t == "MetaString" then return parse_inlines(value.text) end
  local ok, s = pcall(pandoc.utils.stringify, value)
  if ok and s and s ~= "" then return parse_inlines(s) end
  return nil
end
local function meta_to_string(value)
  if value == nil then return nil end
  if type(value) == "string" then return value end
  if type(value) == "boolean" then return value and "true" or "false" end
  if type(value) == "number" then return tostring(value) end
  if type(value) ~= "table" then return nil end
  if value.t == "MetaBool" then
    if value.bool == true or value == true then return "true" end
    if value.bool == false then return "false" end
  end
  local ok, s = pcall(pandoc.utils.stringify, value)
  if ok then return s end
  return nil
end
local function meta_is_true(value)
  if value == true then return true end
  local s = meta_to_string(value)
  return s == "true" or s == "True"
end
local function heading_key(inlines)
  local ok, s = pcall(pandoc.utils.stringify, inlines)
  if not ok or not s then return "" end
  s = s:gsub("%s+", " "):gsub("^%s+", ""):gsub("%s+$", ""):gsub(":%s*$", "")
  return s:lower()
end
local function latex_escape(text)
  if not text then return "" end
  local out = text:gsub("\\", "\\textbackslash{}"):gsub("([{}$&#_%%])", "\\%1")
  return out
end
local function hex_accent(value)
  if not value then return nil end
  local hex = value:gsub("^#", ""):upper()
  if hex:match("^[0-9A-F][0-9A-F][0-9A-F][0-9A-F][0-9A-F][0-9A-F]$") then
    return hex
  end
  return nil
end
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
    elseif el.t == "Emph" or el.t == "Strong" or el.t == "Superscript"
        or el.t == "Subscript" or el.t == "SmallCaps" or el.t == "Strikeout"
        or el.t == "Quoted" or el.t == "Cite" then
      el.content = process_inlines(el.content)
      out[#out + 1] = el
    elseif el.t == "Link" then
      el.content = process_inlines(el.content)
      out[#out + 1] = el
    elseif el.t == "Span" then
      el.content = process_inlines(el.content)
      out[#out + 1] = el
    else
      out[#out + 1] = el
    end
  end
  return out
end
local function first_plain(blocks)
  if not blocks then return nil end
  for _, blk in ipairs(blocks) do
    if blk.t == "Para" or blk.t == "Plain" then return blk end
  end
  return nil
end
local function split_owner_action(item)
  local plain = first_plain(item)
  local inlines = {}
  if plain then
    for _, n in ipairs(plain.content) do inlines[#inlines + 1] = n end
  end
  local owner = {}
  local action = {}
  if #inlines > 0 and inlines[1].t == "Str" and inlines[1].text:sub(1, 1) == "(" then
    local collecting = true
    for i, el in ipairs(inlines) do
      if collecting then
        owner[#owner + 1] = el
        if el.t == "Str" and el.text:find("%)") then
          local text = el.text
          local before, after = text:match("^(.-%))(.*)$")
          if before then
            owner[#owner] = pandoc.Str(before)
            collecting = false
            if after and after:match("%S") then
              action[#action + 1] = pandoc.Str(after:gsub("^%s+", ""))
            end
          end
        end
      else
        if not (#action == 0 and el.t == "Space") then
          action[#action + 1] = el
        end
      end
      if i == #inlines and collecting then
        action = inlines
        owner = {}
      end
    end
    if #owner > 0 then
      if owner[1].t == "Str" then
        owner[1] = pandoc.Str((owner[1].text:gsub("^%(", "")))
      end
      local last = owner[#owner]
      if last.t == "Str" then
        owner[#owner] = pandoc.Str((last.text:gsub("%)$", "")))
      end
    end
  else
    action = inlines
  end
  if #owner == 0 then owner = { pandoc.Str("—") } end
  return process_inlines(owner), process_inlines(action)
end
local function convert_next_steps(items)
  local out = {}
  out[#out + 1] = raw_block("\\BobMeetingStepsBegin{}")
  out[#out + 1] = raw_block("\\BobMeetingStepsHeader{}")
  for _, item in ipairs(items) do
    local owner, action = split_owner_action(item)
    local row = {
      raw_latex("\\BobMeetingOwnerBegin{}"),
    }
    for _, n in ipairs(owner) do row[#row + 1] = n end
    row[#row + 1] = raw_latex("\\BobMeetingOwnerEnd{}")
    row[#row + 1] = raw_latex("\\BobMeetingActionBegin{}")
    for _, n in ipairs(action) do row[#row + 1] = n end
    row[#row + 1] = raw_latex("\\BobMeetingActionEnd{}")
    out[#out + 1] = pandoc.Para(row)
  end
  out[#out + 1] = raw_block("\\BobMeetingStepsEnd{}")
  return out
end
local function emphasize_speaker(inlines)
  local out = {}
  local seen = false
  for _, el in ipairs(inlines) do
    if not seen and el.t == "Str" and el.text:find(":") then
      local name, rest = el.text:match("^(.-:)(.*)$")
      out[#out + 1] = pandoc.Strong({ pandoc.Str(name) })
      if rest ~= "" then out[#out + 1] = pandoc.Str(rest) end
      seen = true
    else
      out[#out + 1] = el
    end
  end
  return process_inlines(out)
end
local function process_transcript_blocks(blocks)
  local out = {}
  for _, blk in ipairs(blocks) do
    if blk.t == "Para" or blk.t == "Plain" then
      blk.content = emphasize_speaker(blk.content)
      out[#out + 1] = blk
    elseif blk.t == "BulletList" or blk.t == "OrderedList" then
      local items = {}
      for _, item in ipairs(blk.content) do
        items[#items + 1] = process_transcript_blocks(item)
      end
      blk.content = items
      out[#out + 1] = blk
    else
      out[#out + 1] = blk
    end
  end
  return out
end
local function has_class(el, name)
  if not el.classes then return false end
  for _, c in ipairs(el.classes) do
    if c == name then return true end
  end
  return false
end
-- Print order is TL;DR, then Next steps, then the remaining sections, then
-- the transcript appendix. The dossier keeps source order; the profile hoists.
local function hoist_next_steps(blocks)
  local tldr = {}
  local next_steps = {}
  local rest = {}
  local i = 1
  while i <= #blocks do
    local blk = blocks[i]
    if blk.t == "Div" and has_class(blk, "meeting-tldr") then
      tldr[#tldr + 1] = blk
    elseif blk.t == "Header"
        and NEXT_STEP_HEADINGS[heading_key(blk.content)]
        and blocks[i + 1]
        and blocks[i + 1].t == "BulletList" then
      next_steps[#next_steps + 1] = blk
      next_steps[#next_steps + 1] = blocks[i + 1]
      i = i + 1
    else
      rest[#rest + 1] = blk
    end
    i = i + 1
  end
  local out = {}
  for _, blk in ipairs(tldr) do out[#out + 1] = blk end
  for _, blk in ipairs(next_steps) do out[#out + 1] = blk end
  for _, blk in ipairs(rest) do out[#out + 1] = blk end
  return out
end
local function process_blocks(blocks)
  local out = {}
  local i = 1
  local in_transcript = false
  while i <= #blocks do
    local blk = blocks[i]
    if blk.t == "Header" then
      local key = heading_key(blk.content)
      blk.content = process_inlines(blk.content)
      if key == "transcript" then
        in_transcript = true
        out[#out + 1] = raw_block("\\BobMeetingAppendixBegin{}")
        out[#out + 1] = raw_block("\\needspace{4\\baselineskip}")
        out[#out + 1] = blk
      elseif NEXT_STEP_HEADINGS[key] and blocks[i + 1] and blocks[i + 1].t == "BulletList" then
        out[#out + 1] = raw_block("\\needspace{4\\baselineskip}")
        out[#out + 1] = blk
        for _, b in ipairs(convert_next_steps(blocks[i + 1].content)) do
          out[#out + 1] = b
        end
        i = i + 1
      else
        out[#out + 1] = raw_block("\\needspace{4\\baselineskip}")
        out[#out + 1] = blk
      end
    elseif blk.t == "Div" and has_class(blk, "meeting-tldr") then
      local content = process_blocks(blk.content)
      out[#out + 1] = raw_block("\\BobMeetingTldrBegin{}")
      for _, b in ipairs(content) do out[#out + 1] = b end
      out[#out + 1] = raw_block("\\BobMeetingTldrEnd{}")
    elseif blk.t == "Div" and has_class(blk, "meeting-transcript") then
      if not in_transcript then
        in_transcript = true
        out[#out + 1] = raw_block("\\BobMeetingAppendixBegin{}")
      end
      for _, b in ipairs(process_transcript_blocks(blk.content)) do
        out[#out + 1] = b
      end
    elseif blk.t == "Para" or blk.t == "Plain" then
      if in_transcript then
        blk.content = emphasize_speaker(blk.content)
      else
        blk.content = process_inlines(blk.content)
      end
      out[#out + 1] = blk
    elseif blk.t == "BulletList" or blk.t == "OrderedList" then
      local items = {}
      for _, item in ipairs(blk.content) do
        items[#items + 1] = process_blocks(item)
      end
      blk.content = items
      out[#out + 1] = blk
    elseif blk.t == "CodeBlock" then
      blk.text = strip_fe0f_text(blk.text)
      out[#out + 1] = blk
    else
      out[#out + 1] = blk
    end
    i = i + 1
  end
  if in_transcript then
    out[#out + 1] = raw_block("\\BobMeetingAppendixEnd{}")
  end
  return out
end
local function para_wrapped(begin_tex, inlines, end_tex)
  local out = { raw_latex(begin_tex) }
  for _, n in ipairs(inlines) do out[#out + 1] = n end
  out[#out + 1] = raw_latex(end_tex)
  return pandoc.Para(out)
end
local function join_meta(parts)
  local joined = {}
  for _, part in ipairs(parts) do
    if part and #part > 0 then
      if #joined > 0 then
        joined[#joined + 1] = pandoc.Space()
        joined[#joined + 1] = pandoc.Str("·")
        joined[#joined + 1] = pandoc.Space()
      end
      for _, n in ipairs(part) do joined[#joined + 1] = n end
    end
  end
  return joined
end
function Pandoc(doc)
  if not FORMAT:match("latex") then return nil end
  local glyph_path = meta_to_string(doc.meta["bob-glyph-table"])
  if glyph_path and glyph_path ~= "" then
    load_glyph_table(glyph_path)
  else
    load_glyph_table("glyphs.lua")
  end
  local accent = hex_accent(meta_to_string(doc.meta["bob-meeting-accent"]))
  local event_title = meta_to_inlines(doc.meta["event_title"])
    or meta_to_inlines(doc.meta["bob-meeting-head"])
    or meta_to_inlines(doc.meta["title"])
    or { pandoc.Str("Meeting") }
  local own_title_block = nil
  if doc.blocks[1] and doc.blocks[1].t == "Header" and doc.blocks[1].level == 1 then
    own_title_block = table.remove(doc.blocks, 1)
  end
  local event_s = heading_key(event_title)
  local dek = nil
  if own_title_block then
    local own_s = heading_key(own_title_block.content)
    if own_s ~= "" and own_s ~= event_s then
      dek = process_inlines(own_title_block.content)
    end
  end
  doc.blocks = process_blocks(hoist_next_steps(doc.blocks))
  local day = meta_to_inlines(doc.meta["bob-meeting-day"])
    or meta_to_inlines(doc.meta["date"])
  local span = meta_to_inlines(doc.meta["span"])
  local duration = meta_to_string(doc.meta["bob-meeting-duration"])
  if span and duration and duration ~= "" then
    span = (function()
      local copy = {}
      for _, n in ipairs(span) do copy[#copy + 1] = n end
      copy[#copy + 1] = pandoc.Space()
      copy[#copy + 1] = pandoc.Str("(" .. duration .. ")")
      return copy
    end)()
  end
  local root = meta_to_inlines(doc.meta["root"])
  local producer = meta_to_inlines(doc.meta["bob-meeting-producer"])
    or meta_to_inlines(doc.meta["producer"])
    or { pandoc.Str("Meeting notes") }
  local notes_by = { pandoc.Str("notes by") , pandoc.Space() }
  for _, n in ipairs(producer) do notes_by[#notes_by + 1] = n end
  local meta_line = join_meta({
    day and process_inlines(day) or nil,
    span and process_inlines(span) or nil,
    root and process_inlines(root) or nil,
    process_inlines(notes_by),
  })
  local head = meta_to_string(doc.meta["bob-meeting-head"])
  if not head or head == "" then
    local ok, s = pcall(pandoc.utils.stringify, event_title)
    if ok then head = s end
  end
  local preamble = {}
  if accent then
    preamble[#preamble + 1] = raw_block("\\definecolor{BobAccent}{HTML}{" .. accent .. "}")
  end
  if head and head ~= "" then
    preamble[#preamble + 1] = raw_block(
      "\\renewcommand{\\BobMeetingHeadLeft}{" .. latex_escape(head) .. "}"
    )
  end
  local pdf_title = meta_to_string(doc.meta["bob-meeting-pdf-title"])
  if pdf_title and pdf_title ~= "" then
    preamble[#preamble + 1] = raw_block(
      "\\AtBeginDocument{\\hypersetup{pdftitle={" .. latex_escape(pdf_title) .. "}}}"
    )
  end
  local card = {}
  card[#card + 1] = raw_block("\\BobMeetingKicker{}")
  card[#card + 1] = para_wrapped(
    "\\BobMeetingTitleBegin{}",
    process_inlines(event_title),
    "\\BobMeetingTitleEnd{}"
  )
  if dek then
    card[#card + 1] = para_wrapped("\\BobMeetingDekBegin{}", dek, "\\BobMeetingDekEnd{}")
  end
  if #meta_line > 0 then
    card[#card + 1] = para_wrapped("\\BobMeetingMetaBegin{}", meta_line, "\\BobMeetingMetaEnd{}")
  end
  card[#card + 1] = raw_block("\\BobMeetingRule{}")
  local filed = meta_to_string(doc.meta["filed_date"]) or ""
  local prov = { raw_latex("\\BobMeetingProvenanceBegin{}") }
  prov[#prov + 1] = pandoc.Str("Notes by")
  prov[#prov + 1] = pandoc.Space()
  for _, n in ipairs(process_inlines(producer)) do prov[#prov + 1] = n end
  if filed ~= "" then
    prov[#prov + 1] = pandoc.Space()
    prov[#prov + 1] = pandoc.Str("·")
    prov[#prov + 1] = pandoc.Space()
    prov[#prov + 1] = pandoc.Str("filed")
    prov[#prov + 1] = pandoc.Space()
    prov[#prov + 1] = pandoc.Str(filed)
    prov[#prov + 1] = pandoc.Space()
    prov[#prov + 1] = pandoc.Str("by bob ref meet")
  else
    prov[#prov + 1] = pandoc.Space()
    prov[#prov + 1] = pandoc.Str("· filed by bob ref meet")
  end
  if meta_is_true(doc.meta["edited"]) then
    prov[#prov + 1] = pandoc.Space()
    prov[#prov + 1] = pandoc.Str("· edited")
  end
  prov[#prov + 1] = pandoc.Space()
  prov[#prov + 1] = pandoc.Str("· structure normalized, wording unchanged")
  prov[#prov + 1] = raw_latex("\\BobMeetingProvenanceEnd{}")
  local new_blocks = {}
  for _, b in ipairs(preamble) do new_blocks[#new_blocks + 1] = b end
  for _, b in ipairs(card) do new_blocks[#new_blocks + 1] = b end
  for _, b in ipairs(doc.blocks) do new_blocks[#new_blocks + 1] = b end
  new_blocks[#new_blocks + 1] = pandoc.Para(prov)
  doc.blocks = new_blocks
  return doc
end
return { { Pandoc = Pandoc } }
