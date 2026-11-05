- The cells last copied with `y` or `Y` in the result grid paste back exactly as the grid
  showed them (NULL stays NULL), in any result in any tab, without reading any clipboard:
  `p` adds them as new rows (each press another copy), `g p` pastes them over the cells at
  the cursor. Each is one step `u` undoes.
- A range pasted into the grid skips a first line that names the columns it lands in (a copy
  with its header line), and says so; `Space d V` reads it again keeping that line.
