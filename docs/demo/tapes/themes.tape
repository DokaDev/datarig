# The built-in themes, chosen in the settings screen (Space ,): the Theme row steps through them
# with l and previews each one live, so every theme shows with its name and the app restyled
# around the dialog. The three that follow the terminal's background (tokyo-night, gruvbox,
# catppuccin) are passed over off camera: each looks like one of its own variants. Esc leaves
# without keeping any of them.
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

Sleep 1s
Space
Type ","
Wait+Screen /Settings/
Wait+Screen /Theme\s+‹ tokyo-night\s+›/
Sleep 800ms
# The cursor starts on the first row; the Theme row is the third.
Type "jj"
Sleep 900ms
Type "l"
Wait+Screen /Theme\s+‹ tokyo-night-day\s+›/
Sleep 1.1s
Type "l"
Wait+Screen /Theme\s+‹ tokyo-night-night\s+›/
Sleep 1.1s
Hide
Type "l"
Wait+Screen /Theme\s+‹ gruvbox\s+›/
Type "l"
Wait+Screen /Theme\s+‹ gruvbox-light\s+›/
Show
Sleep 1.1s
Type "l"
Wait+Screen /Theme\s+‹ gruvbox-dark\s+›/
Sleep 1.1s
Type "l"
Wait+Screen /Theme\s+‹ nord\s+›/
Sleep 1.1s
Type "l"
Wait+Screen /Theme\s+‹ dracula\s+›/
Sleep 1.1s
Type "l"
Wait+Screen /Theme\s+‹ terminal\s+›/
Sleep 1.1s
Type "l"
Wait+Screen /Theme\s+‹ dark\s+›/
Sleep 1.1s
Type "l"
Wait+Screen /Theme\s+‹ light\s+›/
Sleep 1.1s
Type "l"
Wait+Screen /Theme\s+‹ high-contrast\s+›/
Sleep 1.1s
Hide
Type "l"
Wait+Screen /Theme\s+‹ catppuccin\s+›/
Type "l"
Wait+Screen /Theme\s+‹ catppuccin-latte\s+›/
Show
Sleep 1.1s
Type "l"
Wait+Screen /Theme\s+‹ catppuccin-mocha\s+›/
Sleep 1.1s
Escape
Wait+Screen /Enter view cell/
Sleep 1s
