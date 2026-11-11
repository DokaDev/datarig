# A table tab: a condition typed in the line above its grid, then a sort by two columns from the
# grid, then a filter by the value of a cell. Each one is a statement run on the server.
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
# The condition, as typed, is what the server runs.
Type "f"
Sleep 500ms
Type@60ms "status = 'shipped'"
Sleep 600ms
Enter
Wait+Screen /WHERE status = 'shipped'/
Wait+Screen /page 1 \/ \?/
Sleep 1.5s

# Sort by the total, going down: s goes up, then down.
Type "lll"
Sleep 500ms
Type "s"
Wait+Screen /ORDER BY total ASC/
Sleep 1s
Type "s"
Wait+Screen /ORDER BY total DESC/
Sleep 1.5s

# The carrier as a second key.
Type "lll"
Sleep 500ms
Type "S"
Wait+Screen /ORDER BY total DESC, carrier ASC/
Sleep 1.5s

# Filter by the value of the cell under the cursor: it is added to the condition.
Type "F"
Wait+Screen /Filter by carrier/
Sleep 1.5s
Enter
Wait+Screen /WHERE status = 'shipped' AND carrier = 'Heron Express'/
Wait+Screen /ORDER BY total DESC, carrier ASC/
Sleep 2s
Screenshot table-filter.png
Sleep 1s
