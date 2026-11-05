- The result grid shows where the cursor is: its row is tinted, its cell more strongly, and its
  column's header too. A selected range is drawn as one block (the separators between its
  columns included), with its rows' numbers and its columns' headers tinted. New theme tokens
  `grid_cursor_row` and `grid_cursor_header`; a theme file that sets its own background or
  selection without them gets tints made of its own colors.
