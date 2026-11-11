-- bob ref create: swap Mermaid code blocks and local SVG images for
-- rendered PDFs (see docs/highlights-create.md).
--
-- Success wraps the rendered diagram Image in \BobFitBegin{}/\BobFitEnd{}
-- (see diagrams.tex) so tall diagrams scale to the height budget. SVG
-- images get that wrap only when they are the sole inline in their
-- paragraph or figure. A failed SVG is left unchanged so image_guard.lua
-- can apply a sibling raster or placeholder.
--
-- First `--lua-filter` on both Markdown PDF routes. Reads
-- `bob-diagrams-results` from Meta. Wrap the file read and
-- `pandoc.json.decode` in `pcall`. If metadata is absent or unreadable,
-- return nothing changed. This filter must never make pandoc fail.
local results = nil
local swapped_svg_pdfs = {}

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

local function strip_query_fragment(target)
  local query = target:find("?", 1, true)
  local fragment = target:find("#", 1, true)
  local stop = nil
  if query and fragment then
    stop = math.min(query, fragment)
  else
    stop = query or fragment
  end
  if stop then return target:sub(1, stop - 1) end
  return target
end

local function is_local(target)
  if target == "" or target:sub(1, 1) == "#" or target:sub(1, 2) == "//" then
    return false
  end
  if target:match("^[%a][%w+%.%-]*:") then return false end
  return true
end

local function is_local_svg(target)
  if not is_local(target) then return false end
  local path = strip_query_fragment(target)
  return path:lower():match("%.svg$") ~= nil
end

local function lookup_svg_pdf(target)
  if not results then return nil end
  local diagrams = results.diagrams
  if type(diagrams) ~= "table" then return nil end
  local entry = diagrams[pandoc.utils.sha1("svg\0" .. target)]
  if type(entry) ~= "table" then return nil end
  if type(entry.pdf) ~= "string" or entry.pdf == "" then return nil end
  local file = io.open(entry.pdf, "rb")
  if not file then return nil end
  file:close()
  return entry.pdf
end

local function is_whitespace_inline(el)
  return el.t == "Space" or el.t == "SoftBreak" or el.t == "LineBreak"
    or (el.t == "Str" and el.text:match("^%s*$"))
end

local function wrap_solo_rendered_svg(inlines)
  local image_index = nil
  for i = 1, #inlines do
    local el = inlines[i]
    if el.t == "Image" then
      if image_index ~= nil then return nil end
      if not swapped_svg_pdfs[el.src] then return nil end
      image_index = i
    elseif not is_whitespace_inline(el) then
      return nil
    end
  end
  if not image_index then return nil end
  local new = pandoc.List()
  for i = 1, #inlines do
    local el = inlines[i]
    if i == image_index then
      new:insert(pandoc.RawInline("latex", "\\BobFitBegin{}"))
      new:insert(el)
      new:insert(pandoc.RawInline("latex", "\\BobFitEnd{}"))
    else
      new:insert(el)
    end
  end
  return new
end

function Image(image)
  local target = image.src or image.target or ""
  if not is_local_svg(target) then return nil end
  local pdf = lookup_svg_pdf(target)
  if not pdf then return nil end
  image.src = pdf
  swapped_svg_pdfs[pdf] = true
  return image
end

function Para(para)
  local wrapped = wrap_solo_rendered_svg(para.content)
  if not wrapped then return nil end
  para.content = wrapped
  return para
end

function Plain(plain)
  local wrapped = wrap_solo_rendered_svg(plain.content)
  if not wrapped then return nil end
  plain.content = wrapped
  return plain
end

return {
  {Meta = Meta},
  {CodeBlock = CodeBlock, Image = Image},
  {Para = Para, Plain = Plain},
}
