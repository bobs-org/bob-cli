-- bob ref create: collect Mermaid code blocks and local SVG images
-- (see docs/highlights-create.md).
--
-- Throwaway pandoc pass used by the diagrams module. Writes a JSON array of
-- `{id, text}` Mermaid entries and `{id, kind="svg", target}` SVG entries in
-- document order to the path passed via `-M bob-diagrams-manifest=<path>`.
-- Mermaid keys are `pandoc.utils.sha1(cb.text)`. SVG keys are
-- `pandoc.utils.sha1("svg\0" .. target)`. Matches
-- `cb.classes:includes("mermaid")` and local `.svg` Image targets.
local items = {}

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

return {{
  CodeBlock = function(cb)
    if cb.classes:includes("mermaid") then
      cb.text = bob_expand_tabs(cb.text)
      items[#items + 1] = {id = pandoc.utils.sha1(cb.text), text = cb.text}
    end
  end,
  Image = function(image)
    local target = image.src or image.target or ""
    if is_local_svg(target) then
      items[#items + 1] = {
        id = pandoc.utils.sha1("svg\0" .. target),
        kind = "svg",
        target = target,
      }
    end
  end
}, {
  Pandoc = function(doc)
    local manifest_path = nil
    local meta = doc.meta["bob-diagrams-manifest"]
    if meta ~= nil then
      manifest_path = pandoc.utils.stringify(meta)
    end
    if not manifest_path or manifest_path == "" then
      return nil
    end
    local encoded = pandoc.json.encode(items)
    -- Empty Lua tables encode as `{}`; the contract is a JSON array.
    if encoded == "{}" then encoded = "[]" end
    local file = io.open(manifest_path, "w")
    if file then
      file:write(encoded)
      file:close()
    end
  end
}}
