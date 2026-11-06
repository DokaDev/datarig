# A chart of a query's rows: revenue per month and channel as lines (the text column splits the
# values into series), a cursor over the points with their exact values, then the same rows as
# bars. Nothing runs again for the chart.
Require datarig
Source settings.tape

Hide
Type "datarig acme-staging"
Enter
Wait+Screen /Connected to acme-staging/
Tab
Wait+Screen /Ctrl\+E run/
Show

Sleep 800ms
Type "i"
# Each line ends before Enter with no completion open (a trailing space closes it).
Type@45ms "SELECT date_trunc('month', placed_at)::date AS month, channel, sum(total) AS revenue "
Enter
Type@45ms "FROM store.orders GROUP BY 1, 2 ORDER BY 1 "
Escape
Sleep 200ms
Escape
Sleep 500ms
Ctrl+E
Wait+Screen /30 rows/
Sleep 1s

# The Chart tab of the results, zoomed to the whole screen.
Tab
Wait+Screen /Enter view cell/
Sleep 500ms
Type "c"
Wait+Screen /Chart · Line/
Wait+Screen /X month · Y revenue · by channel/
Sleep 1s
Type "z"
Wait+Screen /ZOOM/
Sleep 1.5s
Type "l"
Sleep 400ms
Type "l"
Sleep 400ms
Type "l"
Sleep 400ms
Type "l"
Wait+Screen /▸ 2026-05-01 · app [0-9.]+ · phone [0-9.]+ · web [0-9.]+ · row 13/
Sleep 2s
Screenshot charts.png
Sleep 300ms
Type "1"
Wait+Screen /Chart · Bars/
Wait+Screen /\[Bars\]/
Sleep 3s
