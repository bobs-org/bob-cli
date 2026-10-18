-- bob ref create: collect Mermaid code blocks (see docs/highlights-create.md).
--
-- Throwaway pandoc pass used by the diagrams module. Writes a JSON array of
-- `{id, text}` entries in document order to the path passed via
-- `-M bob-diagrams-manifest=<path>`. Keys are `pandoc.utils.sha1(cb.text)`.
-- Matches `cb.classes:includes("mermaid")` only (A1).
local items = {}

return {{
  CodeBlock = function(cb)
    if cb.classes:includes("mermaid") then
      items[#items + 1] = {id = pandoc.utils.sha1(cb.text), text = cb.text}
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
