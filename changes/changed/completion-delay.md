- The editor's completion popup opens by itself 60 ms after the last key (it waited 120 ms and
  then up to a tenth of a second more for the next timer tick), exactly at that moment. New
  setting `[editor] completion_delay_ms` (`60`, `0` for at once, `30`, `100`, `150`, `250`;
  settings screen and `:set`).
