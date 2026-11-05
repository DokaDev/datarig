# The same screen in a few of the built-in themes.
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
Wait+Screen /editable · store\.products/
Tab
Show

Sleep 1.5s
Type ":set theme=catppuccin-mocha"
Enter
Wait+Screen /Theme: catppuccin-mocha/
Sleep 1.8s
Type ":set theme=gruvbox-light"
Enter
Wait+Screen /Theme: gruvbox-light/
Sleep 1.8s
Type ":set theme=nord"
Enter
Wait+Screen /Theme: nord/
Sleep 1.8s
Type ":set theme=dracula"
Enter
Wait+Screen /Theme: dracula/
Sleep 1.8s
Type ":set theme=tokyo-night"
Enter
Wait+Screen /Theme: tokyo-night/
Sleep 1.5s
