# Themes

One setting, `theme`, picks the colors: `:set theme=nord`, the config file's top-level `theme`
key, or the settings screen (`Space ,`), where moving over the names previews them (`Enter`
keeps one, `Esc` goes back).

```text
{{#include ../screens/settings.txt}}
```

| `theme` | What it does |
|---|---|
| `terminal` | The default: your terminal's own colors and background (its 16 ANSI colors by role) |
| `dark`, `light`, `high-contrast` | datarig's own truecolor themes |
| `catppuccin`, `tokyo-night`, `gruvbox` | The light or the dark variant, following the terminal's background |
| `catppuccin-latte`, `catppuccin-mocha`, `tokyo-night-day`, `tokyo-night-night`, `gruvbox-light`, `gruvbox-dark` | That variant, whatever the background |
| `nord`, `dracula` | Those palettes (dark) |
| any other name | Your theme file `themes/<name>.toml`, next to the config file |

## Background detection

The terminal's background is asked once at startup (OSC 11). A terminal that does not answer
counts as dark, and the settings screen says which it found. This only matters for the
families that follow the background (`catppuccin`, `tokyo-night`, `gruvbox`).

## Your own theme

A theme file sets some tokens and takes the rest from `extends` (a built-in theme; `terminal`
without it). Colors are `#rrggbb`, an ANSI name (`red`, `bright-black`, …) or `default`:

```toml
# themes/mine.toml next to config.toml, used with theme = "mine"
extends = "dark"

[colors]
accent = "#89b4fa"
warning = "yellow"

[styles]
selection = { bg = "#45475a", modifiers = ["bold"] }
```

Every token, with what it colors, is listed on the
[theme file format](../reference/theme-files.md) page. A mistake in the file (an unknown token,
a bad color), an unknown name, or a file named like a built-in theme is reported with the file
and line, and the app draws with `terminal` until it is fixed.

To start a theme file, let datarig write it:

- `:theme copy <from> <name>` writes `themes/<name>.toml` with every token of `<from>` (a
  built-in theme or a theme file) set to its value, each under a comment saying what it colors.
- `:theme new <name>` writes one that extends the current theme (when it is built in, else
  `dark`) with every token commented out, showing the value it inherits.
- On the settings screen, `c` on the theme row copies the theme shown there and asks for a
  name.

None of them replaces a file: a taken name, a built-in theme's name, a Windows device name
(`con`, `nul`, `com1`, …) or a name that is not letters, digits, `-` and `_` is refused. The new
theme is then used and saved, and its path is shown.
