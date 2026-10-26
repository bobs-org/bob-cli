-- Shared by the catalog and Mermaid collect filters. Pandoc parses tabs
-- literally under --preserve-tabs; expand them back to four-column stops
-- after parsing so the rendered AST keeps today's indentation.
function bob_expand_tabs(text)
  local out = {}
  local column = 0
  local index = 1
  while index <= #text do
    local byte = text:sub(index, index)
    if byte == "\t" then
      local spaces = 4 - (column % 4)
      out[#out + 1] = string.rep(" ", spaces)
      column = column + spaces
      index = index + 1
    elseif byte == "\n" then
      out[#out + 1] = byte
      column = 0
      index = index + 1
    elseif byte == "\r" then
      out[#out + 1] = byte
      if text:sub(index + 1, index + 1) ~= "\n" then
        column = 0
      end
      index = index + 1
    else
      local next_index = utf8.offset(text, 2, index)
      local stop = next_index or (#text + 1)
      out[#out + 1] = text:sub(index, stop - 1)
      column = column + 1
      index = stop
    end
  end
  return table.concat(out)
end
