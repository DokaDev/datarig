# A wide table: search the fetched rows, then move across its columns.
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
Wait+Screen /editable · store\.orders/
Tab
Sleep 300ms
# The cell inspector off, for the columns.
Type "i"
Sleep 300ms
Show

Sleep 1s
Type "/Heron"
Sleep 600ms
Enter
Wait+Screen /\/Heron\s+1\/\d+/
Sleep 1s
Type "n"
Sleep 700ms
Type "n"
Wait+Screen /\/Heron\s+3\/\d+/
Sleep 1.2s
Screenshot search.png
Escape
Sleep 600ms
Type "$"
Wait+Screen /gift_note/
Sleep 1.5s
Type "hhh"
Sleep 1s
Type "0"
Sleep 1.5s
