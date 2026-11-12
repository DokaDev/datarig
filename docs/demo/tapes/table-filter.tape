# A table tab: a value ticked in the filter dialog, then only the value of a cell from the grid,
# a column hidden, the filter cleared and brought back with the dialog's undo. Each filter is a
# statement run on the server.
Require datarig
Source settings.tape

Hide
Type "datarig acme-staging"
Enter
Wait+Screen /Connected to acme-staging/
Wait+Screen /store/
Type "jjj"
Type "l"
Wait+Screen /orders/
Type "jjjj"
Enter
Wait+Screen /acme\.store\.orders/
Wait+Screen /Heron Express/
Tab
Wait+Screen /Enter view cell/
Sleep 300ms
# The cell inspector off, for the columns.
Type "i"
Sleep 300ms
Show

Sleep 1s
# The dialog on the status column: its values, counted in the rows loaded.
Type "ll"
Sleep 500ms
Type "f"
Wait+Screen /Filter and sort · store\.orders/
Sleep 1.5s
Right
Sleep 400ms
Down
Wait+Screen /Space tick/
Sleep 1s
# Tick a value: the condition it makes is what the server runs.
Space
Wait+Screen /status = 'delivered' ×/
Sleep 1.5s
Enter
Wait+Screen /╰ \S+ status = 'delivered' ─/
Sleep 1.5s

# Only the carrier of the cell under the cursor.
Type "llll"
Sleep 500ms
Type "="
Wait+Screen /status = 'delivered' \+1 ─/
Sleep 1.5s

# Hide a column from the grid: it is still read.
Type "l"
Sleep 400ms
Type "C"
Wait+Screen /status = 'delivered' \+1 · 1 hidden/
Sleep 1.5s

# Clear the conditions; the dialog's undo brings them back.
Type "X"
Wait+Screen /╰ 1 hidden ─/
Sleep 1.5s
Type "f"
Wait+Screen /Filter and sort · store\.orders/
Sleep 800ms
Type "u"
Wait+Screen /Where .*status = 'delivered' ×/
Sleep 1.5s
Enter
Wait+Screen /status = 'delivered' \+1 · 1 hidden/
Sleep 2s
Screenshot table-filter.png
Sleep 1s
