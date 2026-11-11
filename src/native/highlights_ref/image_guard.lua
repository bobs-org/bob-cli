-- bob ref create: keep SVG image embeds from aborting generic Markdown PDFs.
--
-- A same-stem PNG/JPG/JPEG is used when present; all other SVG images become
-- visible placeholders. File probes and the optional report write are
-- protected so this filter never makes pandoc fail.

local BASE = nil
local REPORT = nil
local DIAGRAMS = nil
local HANDLED = {}

local function meta_text(meta, key)
  local value = meta[key]
  if value == nil then return nil end
  local text = pandoc.utils.stringify(value)
  if text == "" then return nil end
  return text
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

local function percent_decode(target)
  return (target:gsub("%%(%x%x)", function(hex)
    return string.char(tonumber(hex, 16))
  end))
end

local function is_svg(target)
  if target:lower():match("^data:image/svg%+xml") then return true end
  local path = strip_query_fragment(target)
  return path:lower():match("%.svg$") ~= nil
end

local function is_local(target)
  if target == "" or target:sub(1, 1) == "#" or target:sub(1, 2) == "//" then
    return false
  end
  if target:match("^[%a][%w+%.%-]*:") then return false end
  return true
end

local function join(base, path)
  if path:sub(1, 1) == "/" or path:match("^[%a]:[/\\]") then
    return path
  end
  if not base or base == "" then return path end
  local separator = base:sub(-1) == "/" and "" or "/"
  return base .. separator .. path
end

local function exists(path)
  local ok, found = pcall(function()
    local file = io.open(path, "rb")
    if not file then return false end
    file:close()
    return true
  end)
  return ok and found == true
end

local function sibling(target)
  if not is_local(target) then return nil end
  local path = strip_query_fragment(target)
  if path:sub(1, 1) == "#" then return nil end

  local variants = { path }
  local decoded = percent_decode(path)
  if decoded ~= path then variants[#variants + 1] = decoded end

  -- Extension order is part of the capture contract, including when both
  -- raw and percent-decoded source paths have candidate siblings.
  for _, extension in ipairs({ ".png", ".jpg", ".jpeg" }) do
    for _, variant in ipairs(variants) do
      local stem = variant:sub(1, #variant - 4)
      local replacement = stem .. extension
      if exists(join(BASE, replacement)) then return replacement end
    end
  end
  return nil
end

local function renderer_error(target)
  if not DIAGRAMS then return nil end
  local diagrams = DIAGRAMS.diagrams
  if type(diagrams) == "table" then
    local entry = diagrams[pandoc.utils.sha1("svg\0" .. target)]
    if type(entry) == "table"
        and type(entry.error) == "string"
        and entry.error ~= "" then
      return entry.error
    end
  end
  if type(DIAGRAMS.fallback_error) == "string"
      and DIAGRAMS.fallback_error ~= "" then
    return DIAGRAMS.fallback_error
  end
  return nil
end

local function record(target, outcome, replacement)
  local entry = {
    target = target,
    outcome = outcome,
    replacement = replacement,
  }
  local err = renderer_error(target)
  if err then entry.error = err end
  HANDLED[#HANDLED + 1] = entry
end

local function placeholder(target, image)
  local classes = {}
  for _, class in ipairs(image.classes or {}) do
    classes[#classes + 1] = class
  end
  local has_missing = false
  for _, class in ipairs(classes) do
    if class == "missing" then has_missing = true end
  end
  if not has_missing then classes[#classes + 1] = "missing" end
  local attr = pandoc.Attr(
    image.identifier or "",
    classes,
    image.attributes or {}
  )
  local text = "Image not printable · " .. target
  return pandoc.Span({ pandoc.Str(text) }, attr)
end

local function write_report()
  if not REPORT or REPORT == "" then return end
  pcall(function()
    local file = io.open(REPORT, "w")
    if not file then return end
    -- Empty Lua tables encode as `{}`; `images` must stay a JSON array
    -- so a fully vector-rendered document does not warn.
    local encoded = pandoc.json.encode({ version = 1, images = HANDLED })
    if #HANDLED == 0 then
      encoded = '{"version":1,"images":[]}'
    end
    file:write(encoded)
    file:close()
  end)
end

local function handle_image(image)
  local target = image.src or image.target or ""
  if not is_svg(target) then return nil end

  local ok, replacement = pcall(sibling, target)
  if ok and replacement then
    image.src = replacement
    record(target, "sibling", replacement)
    return image
  end

  record(target, "placeholder", "Image not printable · " .. target)
  return placeholder(target, image)
end

function Meta(meta)
  BASE = meta_text(meta, "bob-image-base")
  REPORT = meta_text(meta, "bob-image-guard-report")
  local diagrams_path = meta_text(meta, "bob-diagrams-results")
  if diagrams_path then
    pcall(function()
      local file = io.open(diagrams_path, "r")
      if not file then return end
      local body = file:read("*a")
      file:close()
      local data = pandoc.json.decode(body)
      if type(data) == "table" then DIAGRAMS = data end
    end)
  end
end

function Image(image)
  local ok, result = pcall(handle_image, image)
  if ok then return result end

  -- A malformed local path or unexpected AST property still degrades to
  -- readable text rather than leaving the SVG for pandoc's PDF writer.
  local target = image.src or image.target or ""
  if is_svg(target) then
    record(target, "placeholder", "Image not printable · " .. target)
    return placeholder(target, image)
  end
  return nil
end

function Pandoc(doc)
  write_report()
  return nil
end

return {{ Meta = Meta }, { Image = Image }, { Pandoc = Pandoc }}
