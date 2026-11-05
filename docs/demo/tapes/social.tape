# The repository's social preview (render.sh makes social.png of the still: 1280x640 with the
# name and the tagline above it). The same screen as the hero's still, a pending edit in the
# grid, in a terminal whose shape fits the image; only the end is recorded.
Require datarig
Source settings.tape
Set Columns 140
Set Rows 27

Hide
Type "datarig acme-staging"
Enter
Wait+Screen /Connected to acme-staging/
Wait+Screen /store/
Type "jjj"
Type "l"
Wait+Screen /customers/
Type "jj"
Enter
Wait+Screen /Bram Driftwood/
Tab
Wait+Screen /Enter view cell/
Type "i"
Wait+Screen /Dex Copperkettle/
Type "lll"
Type "e"
Wait+Screen /Enter keep · Esc cancel/
Ctrl+U
Type "Harborview"
Enter
Wait+Screen /1 pending/
Wait+Screen /name\s+│/
Show
Sleep 400ms
Screenshot social-raw.png
Sleep 400ms
