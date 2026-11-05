- New setting `ui.background = "theme" | "terminal"` (settings screen, `:set
  ui.background=terminal`): with `terminal` the panes and the result grid's alternate rows leave
  the terminal's own background showing, so a translucent terminal shows through; text,
  selections and edit marks keep the theme's colors, and dialogs, menus and popups keep its
  surface. Turning it on with a theme that has a background of its own says so in one line.
