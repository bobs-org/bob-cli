-- Last pandoc filter in the catalog pipeline. It sees final section levels
-- (after Bob's top-level shift and return-link edits) and writes the report
-- that Rust combines with the rendered PDF's named destinations.
local RECORDS_KEY = "bob-code-records"
local LINK_RECORDS_KEY = "bob-link-records"
local REPORT_KEY = "bob-code-report"

local function section_numbers(doc)
  local numbers = {}
  local counters = {}
  local enabled = PANDOC_WRITER_OPTIONS
      and PANDOC_WRITER_OPTIONS.number_sections
  doc:walk({
    Header = function(header)
      local level = header.level
      for deeper = level + 1, 6 do counters[deeper] = nil end
      counters[level] = counters[level] or 0
      local number = nil
      if enabled and not header.classes:includes("unnumbered") then
        counters[level] = counters[level] + 1
        local parts = {}
        for index = 1, level do
          parts[#parts + 1] = tostring(counters[index] or 0)
        end
        number = table.concat(parts, ".")
      end
      numbers[#numbers + 1] = number or pandoc.json.null
    end,
  })
  return numbers
end

local function write_report(doc)
  local report_path = doc.meta[REPORT_KEY]
  if report_path == nil then return end
  report_path = pandoc.utils.stringify(report_path)
  if report_path == "" then return end

  local raw_records = doc.meta[RECORDS_KEY]
  if raw_records == nil then return end
  local records = pandoc.json.decode(pandoc.utils.stringify(raw_records))
  local raw_links = doc.meta[LINK_RECORDS_KEY]
  local link_records = {}
  if raw_links ~= nil then
    local ok, decoded =
      pcall(pandoc.json.decode, pandoc.utils.stringify(raw_links))
    if ok and type(decoded) == "table" then link_records = decoded end
  end
  local headings = section_numbers(doc)
  local blocks = {}
  for _, record in ipairs(records) do
    local section = pandoc.json.null
    local header = record.section_header or 0
    if header > 0 then
      section = {
        title = record.section_title or "",
        number = headings[header] or pandoc.json.null,
      }
    end
    blocks[#blocks + 1] = {
      n = record.n,
      kind = record.kind,
      lang = record.lang or pandoc.json.null,
      classes = record.classes or {},
      attrs = record.attrs or {},
      text = record.text or "",
      section = section,
    }
  end
  local links = {}
  for _, record in ipairs(link_records) do
    local section = pandoc.json.null
    local header = record.section_header or 0
    if header > 0 then
      section = {
        title = record.section_title or "",
        number = headings[header] or pandoc.json.null,
      }
    end
    links[#links + 1] = {
      n = record.n,
      url = record.url,
      text = record.text or "",
      title = record.title or pandoc.json.null,
      autolink = record.autolink or false,
      context = record.context or pandoc.json.null,
      section = section,
    }
  end
  local encoded = pandoc.json.encode(
    {version = 1, blocks = blocks, links = links}
  )
  -- Pandoc's JSON encoder represents an empty Lua table as an object. The
  -- contract uses arrays for these fields even when they have no items.
  encoded = encoded:gsub('"classes":{}', '"classes":[]')
  encoded = encoded:gsub('"blocks":{}', '"blocks":[]')
  encoded = encoded:gsub('"links":{}', '"links":[]')
  local file = io.open(report_path, "w")
  if file then
    file:write(encoded)
    file:close()
  end
end

return {{Pandoc = function(doc)
  pcall(write_report, doc)
  return nil
end}}
