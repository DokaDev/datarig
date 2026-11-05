# The same screen in a few of the built-in themes (the commands that switch them are not
# shown: a GIF of whole-screen color changes is large enough already).
Require datarig
Source settings.tape

Hide
Type "datarig acme-staging"
Enter
Wait+Screen /Connected to acme-staging/
Wait+Screen /store/
Type "jjj"
Type "l"
Wait+Screen /products/
Type "jjjjj"
Enter
Wait+Screen /acme\.store\.products/
Wait+Screen /SKU-0001/
Tab
Wait+Screen /Enter view cell/
Show

Sleep 2s
Hide
Type ":set theme=catppuccin-mocha"
Enter
Wait+Screen /Theme: catppuccin-mocha/
Show
Sleep 2s
Hide
Type ":set theme=gruvbox-light"
Enter
Wait+Screen /Theme: gruvbox-light/
Show
Sleep 2s
Hide
Type ":set theme=nord"
Enter
Wait+Screen /Theme: nord/
Show
Sleep 2s
Hide
Type ":set theme=dracula"
Enter
Wait+Screen /Theme: dracula/
Show
Sleep 2s
