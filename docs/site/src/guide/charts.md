# Charts

The Chart tab of the results pane draws the rows a result has fetched as vertical bars,
horizontal bars or lines (in braille dots). Open it with `c` in the grid, `Space r c`, its tab
in the strip, or the action menu. Nothing is asked of the server and nothing runs again; nothing
is read for a chart until its tab is viewed.

```text
{{#include ../screens/chart-bars.txt}}
```

## What it draws

The chart picks its columns at first sight:

- a date or time column and numbers: lines over time;
- a text column and numbers: bars (horizontal with more than 12 of them or labels longer than
  12 characters);
- two or more numbers alone: lines over the first one; a single number alone: bars (lines
  past 40 rows).

Several number columns are several series, and one number with a text column of 2 to 10
values (in the first 500 rows) gives a series per value. The chart says how many fetched rows it draws and whether the server
has more: fetch more pages first to chart them.

- Rows with the same X are summed.
- NULLs are skipped and counted under the chart.
- More than 50 bars or 6 series keep the largest and sum the rest into "others".
- A result without rows or numbers, or with a single point, says why instead of drawing.

## Keys

| Keys | What they do |
|---|---|
| `v` / `V`, `1` `2` `3` | The kind: bars, horizontal bars, line |
| `x` | Choose the X column |
| `s` | Choose the value columns |
| `b` | A column that splits the values into series |
| `S` | Logarithmic scale on or off |
| arrows, `h` `j` `k` `l` | Move a cursor over the bars or points, showing the exact values and their row |
| `Enter` | Show that row in the grid |
| `y` / `Y` | Copy the chart's numbers as TSV / the drawing as text |
| `c` | Back to the rows |

The mouse works too: a click picks a point, a kind or a column choice.

The series' colors are the theme tokens `chart_1` to `chart_6` (see
[theme files](../reference/theme-files.md)).

Not yet: a chart builder that writes the SQL of a chart into a console.
