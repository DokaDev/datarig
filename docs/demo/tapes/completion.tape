# Completion in the editor: a table, then columns of an alias, accepted with Tab and Enter.
# Two of the popups are also a latency check: after the key that opens one (sent alone, with no
# typing pause after it), the popup's own text must show within 120 ms. Calibrated on the hosted
# runners: with the default delay (60 ms) every popup of 160 trials showed within 75-100 ms; a
# 120 ms debounce (the earlier behaviour) cannot show one in 120 ms, and that build and a 250 ms
# delay failed every bound up to 150 ms on the first trial. So a slower popup fails the render.
Require datarig
Source settings.tape

Hide
Type "datarig acme-staging"
Enter
Wait+Screen /Connected to acme-staging/
Tab
Wait+Screen /Ctrl\+E run/
Show

Sleep 800ms
Type "i"
# The first popup also loads the catalog: no latency check here.
Type "SELECT * FROM store.cu"
Wait+Screen /customers\s+table/
Sleep 700ms
Tab
Type " c"
Enter
Type "WHERE c"
Sleep 300ms
Type@0ms "."
Wait+Screen@120ms /newsletter\s+column\s+boolean/
Sleep 900ms
Type "em"
Wait+Screen /email\s+column\s+text/
Sleep 700ms
Tab
Type " LIKE '%7@example.com' AND c"
Sleep 300ms
Type@0ms "."
Wait+Screen@120ms /tier\s+column\s+text/
Sleep 700ms
Type "ti"
Sleep 600ms
Enter
Type " = 'gold' "
Escape
Sleep 200ms
Escape
Sleep 500ms
Ctrl+E
Wait+Screen /\d+ rows?/
Sleep 2.5s
