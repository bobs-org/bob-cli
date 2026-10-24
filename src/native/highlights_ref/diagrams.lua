-- bob ref create: swap Mermaid code blocks for rendered PDFs
-- (see docs/highlights-create.md).
--
-- Success wraps the rendered diagram Image in \BobFitBegin{}/\BobFitEnd{}
-- (see diagrams.tex) so tall diagrams scale to the height budget.
--
-- First `--lua-filter` on both Markdown PDF routes. Reads
-- `bob-diagrams-results` from Meta. Wrap the file read and
-- `pandoc.json.decode` in `pcall`. If metadata is absent or unreadable,
-- return nothing changed. This filter must never make pandoc fail.
local results = nil

local function load_results(meta)
  local value = meta["bob-diagrams-results"]
  if value == nil then return end
  local path = pandoc.utils.stringify(value)
  if path == "" then return end
  local ok, data = pcall(function()
    local file = io.open(path, "r")
    if not file then error("missing results") end
    local body = file:read("*a")
    file:close()
    return pandoc.json.decode(body)
  end)
  if ok and type(data) == "table" then
    results = data
  end
end

local function fallback_reason()
  local err = results and results.fallback_error
  if type(err) == "string" and err ~= "" then return err end
  return "Mermaid renderer unavailable"
end

local function note_para(reason)
  return pandoc.Para{
    pandoc.Emph(pandoc.Inlines("Mermaid diagram not rendered: " .. reason))
  }
end

function Meta(meta)
  load_results(meta)
end

function CodeBlock(cb)
  if not results or not cb.classes:includes("mermaid") then return nil end
  local id = pandoc.utils.sha1(cb.text)
  local diagrams = results.diagrams
  local entry = type(diagrams) == "table" and diagrams[id] or nil
  local pdf = nil
  local reason = nil
  if type(entry) == "table" then
    if type(entry.pdf) == "string" and entry.pdf ~= "" then
      local file = io.open(entry.pdf, "rb")
      if file then
        file:close()
        pdf = entry.pdf
      else
        reason = "rendered PDF missing"
      end
    elseif type(entry.error) == "string" and entry.error ~= "" then
      reason = entry.error
    end
  end
  if pdf then
    local ident = cb.identifier or ""
    return pandoc.Para{
      pandoc.RawInline("latex", "\\BobFitBegin{}"),
      pandoc.Image({}, pdf, "", pandoc.Attr(ident)),
      pandoc.RawInline("latex", "\\BobFitEnd{}")
    }
  end
  return {note_para(reason or fallback_reason()), cb}
end

return {{Meta = Meta}, {CodeBlock = CodeBlock}}
