# The repository's social preview (render.sh makes social.png of the still: 1280x640 with the
# name and the tagline above it): a pending edit in the grid, like the hero's still, in a
# terminal whose shape fits the image; only the end is recorded.
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
Wait+Screen /products/
# products: no other tape changes it (the hero's update moves its row to the end of customers).
Type "jjjjj"
Enter
Wait+Screen /SKU-0001/
Tab
Wait+Screen /Enter view cell/
Type "i"
Wait+Screen /stock/
# The price of the first row.
Type "llll"
Type "e"
Wait+Screen /Enter keep · Esc cancel/
Ctrl+U
Type "52.00"
Enter
Wait+Screen /1 pending/
Wait+Screen /Trail Pack 1/
Wait+Screen /name\s+│/
Show
Sleep 400ms
Screenshot social-raw.png
Sleep 400ms
