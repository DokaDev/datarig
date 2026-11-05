- While a range, rows or columns of the result grid are selected, the status line sums them
  up: their size, then for numbers the sum, average, least and greatest value (integers and
  decimals exactly), else how many distinct values, and how many are NULL. It takes the values
  the grid shows (pending changes too) and reads the rows in the background, so a big selection
  never holds up the keys.
