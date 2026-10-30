-- bob ref create: table-layout pass (see docs/highlights-create.md).
--
-- First `--lua-filter` on both pandoc → XeLaTeX chains. Prose tables become
-- single-column row records; remaining grids get content-aware widths and row air.
-- Classification, override precedence, the complex-table guard, grid sizing,
-- row air, and the JSON report follow the shared contract. Never fails a
-- render: each table rewrite is pcall'd. Writes one JSON report to the path
-- in `-M bob-table-layout-report=<path>`.

local DEFAULT_PROSE_WORDS = 12
local DEFAULT_LINE_CHARS = 84
local CLASS_RECORDS = "bob-records"
local CLASS_GRID = "bob-grid"
local CLASS_WRAPPER = "bob-table-records"

local REASON_SPAN = "row or column spans"
local REASON_HEAD_ROWS = "multiple header rows"
local REASON_BODY_HEAD = "intermediate header row"
local REASON_FOOT = "footer row"
local REASON_BODIES = "multiple table bodies"
local REASON_ROW_HEAD = "row header columns"
local REASON_NESTED = "nested table"
local REASON_HEADER =
  "header contains a link, note, image, or identifier"

local function trim(s)
  return (s:gsub("^%s+", ""):gsub("%s+$", ""))
end

local function word_count(blocks)
  if blocks == nil then return 0 end
  local n = 0
  for _ in pandoc.utils.stringify(blocks):gmatch("%S+") do
    n = n + 1
  end
  return n
end

local function meta_text(meta, key)
  local v = meta[key]
  if v == nil then return nil end
  if type(v) == "boolean" then
    return v and "true" or "false"
  end
  local s = trim(pandoc.utils.stringify(v))
  if s == "" then return nil end
  return s
end

local function parse_layout(s)
  if s == nil then return nil, false end
  local folded = s:lower()
  if folded == "auto" or folded == "records" or folded == "grid" then
    return folded, false
  end
  return nil, true
end

local function parse_prose_words(s)
  if s == nil then return DEFAULT_PROSE_WORDS, false end
  local n = tonumber(s)
  if n == nil or n < 1 or n ~= math.floor(n) then
    return DEFAULT_PROSE_WORDS, true
  end
  return n, false
end

local function has_class(el, name)
  return el.classes ~= nil and el.classes:includes(name)
end

local function clone_inlines(inlines)
  local out = pandoc.List({})
  if inlines == nil then return out end
  for _, el in ipairs(inlines) do
    out:insert(el:clone())
  end
  return out
end

local function cell_blocks(cell)
  if cell == nil or cell.contents == nil then
    return pandoc.List({})
  end
  return cell.contents
end

local function is_empty_blocks(blocks)
  return word_count(blocks) == 0
end

local function is_single_plain_or_para(blocks)
  return blocks ~= nil
    and #blocks == 1
    and (blocks[1].t == "Para" or blocks[1].t == "Plain")
end

local function single_inlines(blocks)
  if is_single_plain_or_para(blocks) then
    return blocks[1].content
  end
  return nil
end

local function body_rows(tbl)
  local rows = {}
  if tbl.bodies == nil then return rows end
  for _, body in ipairs(tbl.bodies) do
    if body.body ~= nil then
      for _, row in ipairs(body.body) do
        rows[#rows + 1] = row
      end
    end
  end
  return rows
end

local function header_row_cells(tbl)
  if tbl.head == nil or tbl.head.rows == nil or #tbl.head.rows == 0 then
    return {}
  end
  return tbl.head.rows[1].cells or {}
end

local function cell_span(cell, field)
  local n = cell and cell[field]
  if type(n) ~= "number" then return 1 end
  return n
end

local function blocks_have_table(blocks)
  if blocks == nil then return false end
  local found = false
  pandoc.walk_block(pandoc.Div(blocks), {
    Table = function()
      found = true
    end,
  })
  return found
end

local function header_cell_is_complex(blocks)
  if blocks == nil then return false end
  local found = false
  local function mark(el)
    if el.identifier ~= nil and el.identifier ~= "" then
      found = true
    end
    if el.t == "Link" or el.t == "Note" or el.t == "Image" then
      found = true
    end
  end
  pandoc.walk_block(pandoc.Div(blocks), {
    Link = mark,
    Note = mark,
    Image = mark,
    Span = mark,
    Code = mark,
    Div = mark,
    Header = mark,
    CodeBlock = mark,
    Figure = mark,
    Table = mark,
  })
  return found
end

local function any_cell(tbl, pred)
  local function rows(section)
    if section == nil or section.rows == nil then return end
    for _, row in ipairs(section.rows) do
      for _, cell in ipairs(row.cells or {}) do
        if pred(cell) then return true end
      end
    end
    return false
  end
  if rows(tbl.head) then return true end
  if rows(tbl.foot) then return true end
  if tbl.bodies == nil then return false end
  for _, body in ipairs(tbl.bodies) do
    if rows({ rows = body.head }) then return true end
    if rows({ rows = body.body }) then return true end
  end
  return false
end

local function char_len(s)
  if utf8 ~= nil and utf8.len ~= nil then
    local n = utf8.len(s)
    if n ~= nil then return n end
  end
  return #s
end

local function has_spans(tbl)
  return any_cell(tbl, function(cell)
    return cell_span(cell, "row_span") > 1
      or cell_span(cell, "col_span") > 1
  end)
end

local function deliberate_widths(tbl)
  local widths = {}
  if tbl.colspecs == nil or #tbl.colspecs < 2 then return false end
  for _, colspec in ipairs(tbl.colspecs) do
    local width = colspec[2]
    if type(width) ~= "number" or width <= 0 then return false end
    widths[#widths + 1] = width
  end
  table.sort(widths)
  return widths[#widths] > widths[1] * 1.1
end

local function cell_widths(tbl)
  local n = tbl.colspecs and #tbl.colspecs or 0
  local mins, maxs = {}, {}
  for i = 1, n do mins[i], maxs[i] = 1, 1 end
  local function scan_row(row)
    for i, cell in ipairs(row.cells or {}) do
      if i <= n and cell_span(cell, "col_span") == 1 then
        local value = pandoc.utils.stringify(cell_blocks(cell))
        maxs[i] = math.max(maxs[i], char_len(value))
        for word in value:gmatch("%S+") do
          mins[i] = math.max(mins[i], math.min(char_len(word), 24))
        end
      end
    end
  end
  if tbl.head ~= nil then
    for _, row in ipairs(tbl.head.rows or {}) do scan_row(row) end
  end
  if tbl.bodies ~= nil then
    for _, body in ipairs(tbl.bodies) do
      for _, row in ipairs(body.head or {}) do scan_row(row) end
      for _, row in ipairs(body.body or {}) do scan_row(row) end
    end
  end
  if tbl.foot ~= nil then
    for _, row in ipairs(tbl.foot.rows or {}) do scan_row(row) end
  end
  return n, mins, maxs
end

local function improve_grid(tbl, ctx)
  -- Pandoc's width-based span representation cannot be adjusted safely when
  -- cells span columns; row air still applies to those tables.
  if not has_spans(tbl) and not deliberate_widths(tbl) then
    local n, mins, maxs = cell_widths(tbl)
    if n > 0 then
      local sum_min, sum_max = 0, 0
      for i = 1, n do
        sum_min = sum_min + mins[i]
        sum_max = sum_max + maxs[i]
      end
      local line_chars = ctx and ctx.line_chars or DEFAULT_LINE_CHARS
      local gutter = 3 * n
      if sum_max + gutter <= line_chars then
        for i = 1, n do
          tbl.colspecs[i] = { tbl.colspecs[i][1], pandoc.ColWidthDefault }
        end
      else
        local available = math.max(line_chars - gutter, sum_min)
        local slack = available - sum_min
        local spread = sum_max - sum_min
        local widths, total = {}, 0
        for i = 1, n do
          local extra = spread > 0
              and slack * (maxs[i] - mins[i]) / spread
            or slack / n
          widths[i] = mins[i] + extra
          total = total + widths[i]
        end
        for i = 1, n do
          tbl.colspecs[i] = {
            tbl.colspecs[i][1],
            widths[i] / total,
          }
        end
      end
    end
  end
  return pandoc.List({
    pandoc.RawBlock("latex", "\\begingroup\\renewcommand{\\arraystretch}{1.2}"),
    tbl,
    pandoc.RawBlock("latex", "\\endgroup"),
  })
end

local function complex_reason_of(tbl)
  if any_cell(tbl, function(cell)
    return cell_span(cell, "row_span") > 1
      or cell_span(cell, "col_span") > 1
  end) then
    return REASON_SPAN
  end
  if tbl.head ~= nil and tbl.head.rows ~= nil and #tbl.head.rows > 1 then
    return REASON_HEAD_ROWS
  end
  if tbl.bodies ~= nil then
    if #tbl.bodies > 1 then return REASON_BODIES end
    for _, body in ipairs(tbl.bodies) do
      if body.head ~= nil and #body.head > 0 then
        return REASON_BODY_HEAD
      end
      local rhc = body.row_head_columns
      if type(rhc) == "number" and rhc > 0 then
        return REASON_ROW_HEAD
      end
    end
  end
  if tbl.foot ~= nil and tbl.foot.rows ~= nil and #tbl.foot.rows > 0 then
    return REASON_FOOT
  end
  if any_cell(tbl, function(cell)
    return blocks_have_table(cell_blocks(cell))
  end) then
    return REASON_NESTED
  end
  if tbl.head ~= nil and tbl.head.rows ~= nil then
    for _, row in ipairs(tbl.head.rows) do
      for _, cell in ipairs(row.cells or {}) do
        if header_cell_is_complex(cell_blocks(cell)) then
          return REASON_HEADER
        end
      end
    end
  end
  return nil
end

local function table_is_prose(tbl, n)
  if tbl.bodies == nil then return false end
  for _, body in ipairs(tbl.bodies) do
    if body.body ~= nil then
      for _, row in ipairs(body.body) do
        for i, cell in ipairs(row.cells or {}) do
          if i > 1 and word_count(cell_blocks(cell)) >= n then
            return true
          end
        end
      end
    end
  end
  return false
end

local function lead_in_cell(cell)
  local blocks = cell_blocks(cell)
  if is_empty_blocks(blocks) then return true end
  if is_single_plain_or_para(blocks) then
    return word_count(blocks) <= 8
  end
  return false
end

local function uses_lead_in(rows)
  if #rows == 0 then return false end
  for _, row in ipairs(rows) do
    if not lead_in_cell((row.cells or {})[1]) then
      return false
    end
  end
  return true
end

local function header_label_inlines(cell)
  local blocks = cell_blocks(cell)
  if is_empty_blocks(blocks) then return nil end
  local single = single_inlines(blocks)
  if single ~= nil then
    return clone_inlines(single)
  end
  return clone_inlines(pandoc.utils.blocks_to_inlines(blocks))
end

local function emdash_inlines()
  return pandoc.List({ pandoc.Str("—") })
end

local function label_open()
  return pandoc.RawInline("latex", "{\\BobRecordLabel ")
end

local function label_close()
  return pandoc.RawInline("latex", "}")
end

local function labeled_inlines(label, value)
  local inl = pandoc.List({})
  inl:insert(label_open())
  for _, el in ipairs(label) do
    inl:insert(el)
  end
  inl:insert(pandoc.Str(":"))
  inl:insert(label_close())
  inl:insert(pandoc.Space())
  for _, el in ipairs(value) do
    inl:insert(el)
  end
  return inl
end

local function label_only_para(label)
  local inl = pandoc.List({})
  inl:insert(label_open())
  for _, el in ipairs(label) do
    inl:insert(el)
  end
  inl:insert(pandoc.Str(":"))
  inl:insert(label_close())
  return pandoc.Para(inl)
end

local function append_value_env(out, blocks)
  out:insert(pandoc.RawBlock("latex", "\\begin{BobRecordValue}"))
  for _, block in ipairs(blocks) do
    out:insert(block)
  end
  out:insert(pandoc.RawBlock("latex", "\\end{BobRecordValue}"))
end

local function empty_value_blocks()
  return { pandoc.Plain(emdash_inlines()) }
end

local function emit_field(out, label, cell, unlabeled_env)
  local blocks = cell_blocks(cell)
  local empty = is_empty_blocks(blocks)
  local single = single_inlines(blocks)
  if unlabeled_env or label == nil then
    if empty then
      append_value_env(out, empty_value_blocks())
    elseif single ~= nil then
      append_value_env(out, { pandoc.Para(single) })
    else
      append_value_env(out, blocks)
    end
    return
  end
  if empty or single ~= nil then
    local value = empty and emdash_inlines() or single
    out:insert(pandoc.Para(labeled_inlines(label, value)))
    return
  end
  out:insert(label_only_para(label))
  append_value_env(out, blocks)
end

local function caption_inlines(tbl)
  if tbl.caption == nil or tbl.caption.long == nil
      or #tbl.caption.long == 0 then
    return nil
  end
  if is_empty_blocks(tbl.caption.long) then return nil end
  return pandoc.utils.blocks_to_inlines(tbl.caption.long)
end

local function to_records(tbl)
  local rows = body_rows(tbl)
  local heads = header_row_cells(tbl)
  local ncol = 0
  if tbl.colspecs ~= nil then ncol = #tbl.colspecs end
  for _, row in ipairs(rows) do
    local n = row.cells and #row.cells or 0
    if n > ncol then ncol = n end
  end
  if #heads > ncol then ncol = #heads end

  local lead = uses_lead_in(rows)
  local two_col_lead = lead and ncol == 2
  local out = pandoc.List({})
  out:insert(pandoc.RawBlock("latex", "\\begin{BobTableRecords}"))

  local cap = caption_inlines(tbl)
  if cap ~= nil then
    local inl = pandoc.List({})
    inl:insert(pandoc.RawInline("latex", "\\BobTableCaption{}"))
    inl:insert(pandoc.Space())
    for _, el in ipairs(cap) do
      inl:insert(el)
    end
    out:insert(pandoc.Para(inl))
  end

  for _, row in ipairs(rows) do
    local cells = row.cells or {}
    out:insert(pandoc.RawBlock("latex", "\\BobRecordStart"))
    if lead then
      local first = cells[1]
      if first ~= nil and not is_empty_blocks(cell_blocks(first)) then
        local inl = single_inlines(cell_blocks(first))
        if inl == nil then
          inl = pandoc.utils.blocks_to_inlines(cell_blocks(first))
        end
        out:insert(pandoc.Para({ pandoc.Strong(inl) }))
      end
    end
    local start = lead and 2 or 1
    for i = start, ncol do
      local unlabeled = two_col_lead and i == 2
      local label = unlabeled and nil or header_label_inlines(heads[i])
      emit_field(out, label, cells[i], unlabeled)
    end
  end

  out:insert(pandoc.RawBlock("latex", "\\end{BobTableRecords}"))

  local classes = {}
  if tbl.classes ~= nil then
    for _, class in ipairs(tbl.classes) do
      classes[#classes + 1] = class
    end
  end
  classes[#classes + 1] = CLASS_WRAPPER
  local ident = tbl.identifier or ""
  local attributes = tbl.attributes
  return pandoc.Div(out, pandoc.Attr(ident, classes, attributes))
end

function Pandoc(doc)
  local warnings = {}
  local function add_warning(msg)
    warnings[#warnings + 1] = msg
  end

  local function safe_improve_grid(tbl, ctx, idx)
    local ok, result = pcall(improve_grid, tbl, ctx)
    if ok and result ~= nil then return result end
    local err = ok and "empty result" or tostring(result)
    add_warning(
      string.format(
        "table %d grid improvement failed (%s); leaving as a grid",
        idx,
        err
      )
    )
    return tbl
  end

  local report_path = meta_text(doc.meta, "bob-table-layout-report")

  local function write_report(obj)
    if report_path == nil or report_path == "" then return end
    local encoded = pandoc.json.encode(obj)
    if type(encoded) ~= "string" then return end
    local file = io.open(report_path, "w")
    if file then
      file:write(encoded)
      file:close()
    end
  end

  local doc_raw = meta_text(doc.meta, "bob-tables")
  local doc_layout, doc_invalid = parse_layout(doc_raw)
  if doc_invalid then
    add_warning(
      "bob-tables: unknown value '"
        .. doc_raw
        .. "' (use auto, records, or grid)"
    )
  end
  local default_raw = meta_text(doc.meta, "bob-tables-default")
  local default_layout, default_invalid = parse_layout(default_raw)
  if default_invalid then
    add_warning(
      "bob-tables-default: unknown value '"
        .. default_raw
        .. "' (use auto, records, or grid)"
    )
  end
  local prose_raw = meta_text(doc.meta, "bob-table-prose-words")
  local prose_words, prose_invalid = parse_prose_words(prose_raw)
  if prose_invalid then
    add_warning(
      "bob-table-prose-words: ignoring '"
        .. tostring(prose_raw)
        .. "' (using "
        .. tostring(DEFAULT_PROSE_WORDS)
        .. ")"
    )
  end
  local line_raw = meta_text(doc.meta, "bob-table-line-chars")
  local line_chars = tonumber(line_raw) or DEFAULT_LINE_CHARS
  if line_chars < 1 or line_chars ~= math.floor(line_chars) then
    line_chars = DEFAULT_LINE_CHARS
  end

  local effective = doc_layout or default_layout or "auto"
  local stack = { effective }
  local table_index = 0
  local tables_n = 0
  local records_n = 0
  local grids_n = 0
  local kept_grid = {}

  local filter = { traverse = "topdown" }

  filter.Div = function(div)
    local records = has_class(div, CLASS_RECORDS)
    local grid = has_class(div, CLASS_GRID)
    local pushed
    if records and grid then
      add_warning(
        "bob-records and bob-grid on the same Div; ignoring both"
      )
      pushed = stack[#stack]
    elseif records then
      pushed = "records"
    elseif grid then
      pushed = "grid"
    else
      pushed = stack[#stack]
    end
    stack[#stack + 1] = pushed
    local walked = pandoc.Pandoc(div.content):walk(filter)
    div.content = walked.blocks
    stack[#stack] = nil
    return div, false
  end

  filter.Table = function(tbl)
    table_index = table_index + 1
    local idx = table_index
    local requested = stack[#stack] or "auto"
    local rows = body_rows(tbl)
    tables_n = tables_n + 1
    if #rows == 0 then
      grids_n = grids_n + 1
      return safe_improve_grid(tbl, {
        requested = requested,
        line_chars = line_chars,
      }, idx)
    end
    local reason = complex_reason_of(tbl)
    local want_records
    if requested == "records" then
      want_records = true
    elseif requested == "grid" then
      want_records = false
    else
      want_records = table_is_prose(tbl, prose_words)
    end
    if want_records and reason == nil then
      local ok, result = pcall(to_records, tbl)
      if ok and result ~= nil then
        records_n = records_n + 1
        if result.t == "Div" then
          local walked = pandoc.Pandoc(result.content):walk(filter)
          result.content = walked.blocks
          return result, false
        end
        return result
      end
      local err = ok and "empty result" or tostring(result)
      add_warning(
        string.format(
          "table %d rewrite failed (%s); leaving as a grid",
          idx,
          err
        )
      )
      grids_n = grids_n + 1
      return safe_improve_grid(tbl, {
        requested = requested,
        line_chars = line_chars,
      }, idx)
    end
    grids_n = grids_n + 1
    if want_records and reason ~= nil then
      local reported = requested == "records" and "records" or "auto"
      kept_grid[#kept_grid + 1] = {
        table = idx,
        reason = reason,
        requested = reported,
      }
    end
    return safe_improve_grid(tbl, {
      requested = requested,
      reason = reason,
      line_chars = line_chars,
    }, idx)
  end

  doc = doc:walk(filter)

  local report = {
    version = 1,
    tables = tables_n,
    records = records_n,
    grids = grids_n,
  }
  if #kept_grid > 0 then report.kept_grid = kept_grid end
  if #warnings > 0 then report.warnings = warnings end
  write_report(report)
  return doc
end
