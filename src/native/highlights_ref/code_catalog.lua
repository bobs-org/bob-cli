-- First pandoc filter in the code-catalog pipeline. Records the parser's
-- original code text and section association, then expands tabs and adds
-- invisible PDF destinations around each block. Also records every URI link
-- (exact target, text, autolink flag, lead-in context, section) with an
-- invisible inline PDF anchor for its page.
local RECORDS_KEY = "bob-code-records"
local LINK_RECORDS_KEY = "bob-link-records"
local records = {}
local link_records = {}
local headers = {}
local header_index = 0

local function is_uri_target(target)
  if type(target) ~= "string" then return false end
  return target:match("^[A-Za-z][A-Za-z0-9%+%.%-]+:") ~= nil
end

local function safe_stringify(inlines)
  local ok, result = pcall(pandoc.utils.stringify, inlines)
  if ok and type(result) == "string" then return result end
  return ""
end

local function collapse_ws(text)
  if type(text) ~= "string" then return "" end
  local collapsed = text:gsub("%s+", " ")
  collapsed = collapsed:gsub("^%s+", ""):gsub("%s+$", "")
  return collapsed
end

local function utf8_tail(text, keep)
  local len = utf8.len(text)
  if not len or len <= keep then return text end
  local start_byte = utf8.offset(text, len - keep + 1)
  if not start_byte then return text end
  return text:sub(start_byte)
end

local function make_context(raw)
  local collapsed = collapse_ws(raw or "")
  if collapsed == "" then return nil end
  if utf8.len(collapsed) and utf8.len(collapsed) > 160 then
    local tail = utf8_tail(collapsed, 160)
    local space_at = tail:find(" ")
    if space_at then tail = tail:sub(space_at + 1) end
    collapsed = "…" .. tail
  elseif #collapsed > 160 then
    local tail = collapsed:sub(-160)
    local space_at = tail:find(" ")
    if space_at then tail = tail:sub(space_at + 1) end
    collapsed = "…" .. tail
  end
  return collapsed
end

local function has_link_class(link, name)
  local ok, result = pcall(function()
    return link.classes:includes(name)
  end)
  return ok and result or false
end

local function record_block(code)
  local classes = {}
  for _, class in ipairs(code.classes) do
    classes[#classes + 1] = class
  end
  local attributes = {}
  for key, value in pairs(code.attributes) do
    attributes[key] = value
  end
  local n = #records + 1
  local lang = classes[1]
  records[n] = {
    n = n,
    kind = code.classes:includes("mermaid") and "mermaid" or "code",
    lang = lang or pandoc.json.null,
    classes = classes,
    attrs = attributes,
    text = code.text,
    section_title = header_index > 0 and headers[header_index] or nil,
    section_header = header_index,
  }
  code.text = bob_expand_tabs(code.text)
  return code, n
end

local function record_link(link, context_raw)
  local target = link.target or ""
  if not is_uri_target(target) then return nil end
  local text = collapse_ws(safe_stringify(link.content))
  local title = nil
  if type(link.title) == "string" and link.title ~= "" then
    title = link.title
  end
  local autolink = has_link_class(link, "uri")
    or has_link_class(link, "email")
  if not autolink then
    if text == target then
      autolink = true
    elseif type(target) == "string" and target:sub(1, 7) == "mailto:" then
      local email = target:sub(8)
      if text == email then autolink = true end
    end
  end
  local n = #link_records + 1
  link_records[n] = {
    n = n,
    url = target,
    text = text,
    title = title,
    autolink = autolink,
    context = make_context(context_raw),
    section_title = header_index > 0 and headers[header_index] or nil,
    section_header = header_index,
  }
  return n
end

local function is_latex()
  return FORMAT and type(FORMAT) == "string" and FORMAT:match("latex") ~= nil
end

-- Walk a list of inlines in document order. `buf` is a mutable
-- `{text = ""}` accumulator for the enclosing Para/Plain/Header/LineBlock
-- so `context` sees every plain word before the link. Returns a new inline
-- list with an invisible anchor before each anchorable link.
local function walk_inline_list(inlines, in_moving, buf)
  local out = {}
  local latex = is_latex()
  for _, inline in ipairs(inlines) do
    local tag = inline.t
    if tag == "Link" then
      local target = inline.target or ""
      if is_uri_target(target) then
        local n = record_link(inline, buf.text)
        if n and not in_moving and latex then
          out[#out + 1] = pandoc.RawInline(
            "latex",
            "\\BobLinkAnchor{bob-link-" .. n .. "}"
          )
        end
        out[#out + 1] = inline
        local link_text = collapse_ws(safe_stringify(inline.content))
        if link_text ~= "" then
          if buf.text == "" then
            buf.text = link_text
          else
            buf.text = buf.text .. " " .. link_text
          end
        end
      else
        out[#out + 1] = inline
        local skipped = collapse_ws(safe_stringify(inline.content))
        if skipped ~= "" then
          if buf.text == "" then
            buf.text = skipped
          else
            buf.text = buf.text .. " " .. skipped
          end
        end
      end
    elseif tag == "Note" then
      local ok, new_blocks = pcall(function()
        return walk_blocks(inline.content, false)
      end)
      if ok and new_blocks then
        inline.content = new_blocks
      end
      out[#out + 1] = inline
    elseif tag == "Image" then
      local alt = collapse_ws(safe_stringify(inline.caption))
      if alt ~= "" then
        if buf.text == "" then
          buf.text = alt
        else
          buf.text = buf.text .. " " .. alt
        end
      end
      out[#out + 1] = inline
    elseif tag == "Code" then
      local ok = pcall(function()
        inline.text = bob_expand_tabs(inline.text)
      end)
      if ok then
        local code_text = collapse_ws(inline.text or "")
        if code_text ~= "" then
          if buf.text == "" then
            buf.text = code_text
          else
            buf.text = buf.text .. " " .. code_text
          end
        end
      end
      out[#out + 1] = inline
    elseif tag == "Emph"
      or tag == "Strong"
      or tag == "Strikeout"
      or tag == "Superscript"
      or tag == "Subscript"
      or tag == "SmallCaps"
      or tag == "Span"
      or tag == "Quoted"
      or tag == "Underline" then
      local ok, rebuilt = pcall(function()
        local inner = walk_inline_list(inline.content, in_moving, buf)
        inline.content = inner
        return inline
      end)
      out[#out + 1] = (ok and rebuilt) or inline
    elseif tag == "Cite" then
      local ok, rebuilt = pcall(function()
        if inline.content then
          inline.content = walk_inline_list(
            inline.content,
            in_moving,
            buf
          )
        end
        if inline.citations then
          for _, citation in ipairs(inline.citations) do
            if citation.prefix then
              citation.prefix = walk_inline_list(
                citation.prefix,
                in_moving,
                buf
              )
            end
            if citation.suffix then
              citation.suffix = walk_inline_list(
                citation.suffix,
                in_moving,
                buf
              )
            end
          end
        end
        return inline
      end)
      out[#out + 1] = (ok and rebuilt) or inline
    else
      local piece = safe_stringify(inline)
      -- RawInline/Str/Space/SoftBreak/LineBreak/Math contribute their
      -- visible words to the lead-in context; other leaves stringify to
      -- "" and leave the buffer unchanged.
      if piece ~= "" and tag ~= "RawInline" then
        piece = collapse_ws(piece)
        if piece ~= "" then
          if buf.text == "" then
            buf.text = piece
          else
            buf.text = buf.text .. " " .. piece
          end
        end
      end
      out[#out + 1] = inline
    end
  end
  return out
end

local function walk_para_like(block, in_moving)
  local buf = {text = ""}
  local ok, inner = pcall(function()
    return walk_inline_list(block.content, in_moving, buf)
  end)
  if ok and inner then block.content = inner end
  return block
end

function walk_blocks(blocks, in_moving)
  local out = {}
  for _, block in ipairs(blocks) do
    local tag = block.t
    if tag == "Header" then
      header_index = header_index + 1
      pcall(function()
        headers[header_index] = pandoc.utils.stringify(block.content)
      end)
      -- Headings are moving arguments: record their links but anchor none.
      local ok, inner = pcall(function()
        local buf = {text = ""}
        return walk_inline_list(block.content, true, buf)
      end)
      if ok and inner then block.content = inner end
      out[#out + 1] = block
    elseif tag == "Para" or tag == "Plain" then
      out[#out + 1] = walk_para_like(block, in_moving)
    elseif tag == "LineBlock" then
      local buf = {text = ""}
      local ok = pcall(function()
        local lines = {}
        for _, line in ipairs(block.content) do
          lines[#lines + 1] = walk_inline_list(line, in_moving, buf)
        end
        block.content = lines
      end)
      out[#out + 1] = block
    elseif tag == "CodeBlock" then
      local ok, result = pcall(function()
        local rewritten, n = record_block(block)
        if not FORMAT:match("latex") then return rewritten end
        return {
          pandoc.RawBlock("latex", "\\BobCodeAnchor{bob-code-" .. n .. "}"),
          rewritten,
          pandoc.RawBlock(
            "latex",
            "\\BobCodeEndAnchor{bob-code-" .. n .. "-end}"
          ),
        }
      end)
      if ok then
        if result and result.t then
          out[#out + 1] = result
        elseif type(result) == "table" and result[1] then
          for _, piece in ipairs(result) do
            out[#out + 1] = piece
          end
        else
          out[#out + 1] = block
        end
      else
        out[#out + 1] = block
      end
    elseif tag == "Code" then
      local ok = pcall(function()
        block.text = bob_expand_tabs(block.text)
      end)
      out[#out + 1] = block
    elseif tag == "BlockQuote" or tag == "Div" then
      local ok, inner = pcall(function()
        return walk_blocks(block.content, in_moving)
      end)
      if ok and inner then block.content = inner end
      out[#out + 1] = block
    elseif tag == "BulletList" or tag == "OrderedList" then
      local ok = pcall(function()
        local items = {}
        for _, item in ipairs(block.content) do
          items[#items + 1] = walk_blocks(item, in_moving)
        end
        block.content = items
      end)
      out[#out + 1] = block
    elseif tag == "DefinitionList" then
      local ok = pcall(function()
        local items = {}
        for _, item in ipairs(block.content) do
          local term, definitions = item[1], item[2]
          local new_definitions = {}
          for _, definition in ipairs(definitions) do
            new_definitions[#new_definitions + 1] = walk_blocks(
              definition,
              in_moving
            )
          end
          items[#items + 1] = {term, new_definitions}
        end
        block.content = items
      end)
      out[#out + 1] = block
    elseif tag == "Table" then
      local ok = pcall(function()
        -- Body rows first (document order for the grid), caption last.
        if block.head and block.head.rows then
          for _, row in ipairs(block.head.rows) do
            for _, cell in ipairs(row.cells) do
              cell.contents = walk_blocks(cell.contents, true)
            end
          end
        end
        if block.bodies then
          for _, body in ipairs(block.bodies) do
            if body.head then
              for _, row in ipairs(body.head) do
                for _, cell in ipairs(row.cells) do
                  cell.contents = walk_blocks(cell.contents, true)
                end
              end
            end
            if body.body then
              for _, row in ipairs(body.body) do
                for _, cell in ipairs(row.cells) do
                  cell.contents = walk_blocks(cell.contents, in_moving)
                end
              end
            end
          end
        end
        if block.foot and block.foot.rows then
          for _, row in ipairs(block.foot.rows) do
            for _, cell in ipairs(row.cells) do
              cell.contents = walk_blocks(cell.contents, in_moving)
            end
          end
        end
        if block.caption and block.caption.long then
          block.caption.long = walk_blocks(block.caption.long, true)
        end
      end)
      out[#out + 1] = block
    elseif tag == "Figure" then
      local ok = pcall(function()
        if block.content then
          block.content = walk_blocks(block.content, in_moving)
        end
        if block.caption and block.caption.long then
          block.caption.long = walk_blocks(block.caption.long, true)
        end
      end)
      out[#out + 1] = block
    else
      out[#out + 1] = block
    end
  end
  return out
end

return {{
  Pandoc = function(doc)
    pcall(function()
      doc.blocks = walk_blocks(doc.blocks, false)
    end)
    pcall(function()
      local encoded = pandoc.json.encode(records)
      if type(encoded) == "string" then doc.meta[RECORDS_KEY] = encoded end
    end)
    pcall(function()
      local encoded_links = pandoc.json.encode(link_records)
      if type(encoded_links) == "string" then
        doc.meta[LINK_RECORDS_KEY] = encoded_links
      end
    end)
    return doc
  end,
}}
