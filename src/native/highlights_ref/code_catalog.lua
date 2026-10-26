-- First pandoc filter in the code-catalog pipeline. Records the parser's
-- original code text and section association, then expands tabs and adds
-- invisible PDF destinations around each block.
local RECORDS_KEY = "bob-code-records"
local records = {}
local headers = {}
local header_index = 0

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

return {{
  Header = function(header)
    header_index = header_index + 1
    pcall(function()
      headers[header_index] = pandoc.utils.stringify(header.content)
    end)
    return header
  end,
  Code = function(code)
    local ok = pcall(function()
      code.text = bob_expand_tabs(code.text)
    end)
    if not ok then return code end
    return code
  end,
  CodeBlock = function(code)
    local ok, result = pcall(function()
      local rewritten, n = record_block(code)
      if not FORMAT:match("latex") then return rewritten end
      return {
        pandoc.RawBlock("latex", "\\BobCodeAnchor{bob-code-" .. n .. "}"),
        rewritten,
        pandoc.RawBlock("latex", "\\BobCodeEndAnchor{bob-code-" .. n .. "-end}"),
      }
    end)
    return ok and result or code
  end,
}, {
  Pandoc = function(doc)
    pcall(function()
      local encoded = pandoc.json.encode(records)
      if type(encoded) == "string" then doc.meta[RECORDS_KEY] = encoded end
    end)
    return doc
  end,
}}
