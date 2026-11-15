# A table tab's query bar: a value ticked in the status column's popup, the order set in the
# order popup, a column hidden in the columns popup, and the condition taken out on the bar.
# Each change is a statement run on the server, and the bar says what is applied.
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
Wait+Screen /Heron Express/
Tab
Wait+Screen /Enter view cell/
Sleep 300ms
# The cell inspector off, for the columns.
Type "i"
Sleep 300ms
Show

Sleep 1s
# The status column's popup (a click on its header opens it too): its conditions, its values
# counted in the rows loaded, and its sort.
Type "ll"
Sleep 500ms
Type "F"
Wait+Screen /╭─ status +text/
Sleep 1.5s
Down
Wait+Screen /▸ \[ \] delivered/
Sleep 1s
# Tick a value, and Enter runs it: the chip on the bar is the text the server runs.
Space
Wait+Screen /\[x\] delivered/
Sleep 1.5s
Enter
Wait+Screen /WHERE +status = 'delivered' ×/
Sleep 2s

# The order popup: the clause it sends underneath.
Space
Type "ro"
Wait+Screen /╭─ ORDER BY ─/
Sleep 1s
Type "jjj"
Sleep 500ms
Space
Sleep 500ms
Space
Wait+Screen /ORDER BY .total. DESC/
Sleep 1.5s
Enter
Wait+Screen /ORDER( BY)? ↓ total \+/
Sleep 2s

# Hide a column from the grid in the columns popup: it is still read.
Space
Type "rc"
Wait+Screen /╭─ columns 14.14/
Sleep 1s
Type "j"
Sleep 500ms
Space
Wait+Screen /╭─ columns 13.14/
Sleep 1s
Enter
Wait+Screen /13.14 cols/
Sleep 2s
Screenshot table-filter.png

# The keys on the bar's conditions: x takes the chip out and runs the others.
Space
Type "rw"
Wait+Screen /x remove/
Sleep 1.5s
Type "x"
Wait+Screen /WHERE \+ │ ORDER( BY)? ↓ total/
Sleep 2.5s
