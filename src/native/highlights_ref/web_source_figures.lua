-- GFM has no figure syntax, so pandoc writes a Figure block as raw HTML
-- and the code blocks and images inside it never reach the catalog.
-- Replace each figure with its content followed by its caption.
function Figure(figure)
  local blocks = pandoc.List(figure.content)
  local caption = figure.caption.long
  if caption and #caption > 0 then
    blocks:extend(caption)
  end
  return blocks
end
