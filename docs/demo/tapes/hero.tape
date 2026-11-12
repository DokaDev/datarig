# The README's first recording: open a table, edit a cell, review the exact SQL in the Changes
# tab and apply it. Every Wait checks the screen; a recording that no longer matches datarig
# fails (docs/demo/render.sh).
Require datarig
Source settings.tape

Hide
Type "datarig acme-staging"
Enter
Wait+Screen /Connected to acme-staging/
Wait+Screen /store/
Show

Sleep 1s
# The explorer's cursor is on the profile: its database, then the `store` schema.
Type "jjj"
Sleep 400ms
Type "l"
Wait+Screen /customers/
Sleep 600ms
Type "jj"
Sleep 400ms
Enter
Wait+Screen /Bram Driftwood/
Sleep 1.2s

# Into the grid, with the cell inspector off so every column fits whole (next to it, moving to
# `city` would scroll the grid sideways and clip the names), to the `city` of the first row.
Tab
Wait+Screen /Enter view cell/
Type "i"
Wait+Screen /newsletter/
Wait+Screen /Dex Copperkettle/
Sleep 300ms
Type "lll"
Sleep 500ms
Type "e"
Wait+Screen /Enter keep · Esc cancel/
Ctrl+U
Type "Harborview"
Sleep 300ms
Enter
Wait+Screen /1 pending/
# Still unscrolled: whole names and the name header.
Wait+Screen /Dex Copperkettle/
Wait+Screen /name\s+│/
Sleep 600ms
Screenshot hero.png
Sleep 600ms

# The pending change, then the statements that will run.
Space
Type "dp"
Wait+Screen /Changes: Result 1 · store\.customers/
Sleep 1.5s
Type "s"
Wait+Screen /UPDATE "store"\."customers" SET "city" = 'Harborview'/
Sleep 2.5s

# Apply: the staging environment asks first, with the SQL in front of you.
Space
Type "da"
Wait+Screen /Apply changes: store\.customers/
Sleep 2.5s
Type "y"
Wait+Screen /Applied 1 change/
Sleep 3s
