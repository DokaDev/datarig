# Profiles with icons and environment marks; a production profile is read-only and a write is
# refused before anything is sent.
Require datarig
Source settings.tape

Hide
Type "datarig"
Enter
Wait+Screen /No open tabs/
Show

Sleep 1.5s
# The cursor starts on the first profile; billing-prod (MySQL) is the third.
Type "jj"
Sleep 600ms
Enter
Wait+Screen /Connected to billing-prod · TLS [0-9.]+, server verified/
Sleep 800ms
Type "j"
Sleep 300ms
Type "l"
Wait+Screen /invoices/
Sleep 1.5s
Screenshot environments.png

Tab
Wait+Screen /Ctrl\+E run/
Sleep 500ms
Type "i"
Type "UPDATE invoices SET status = 'void' WHERE id = 42 "
Escape
Sleep 200ms
Escape
Sleep 500ms
Ctrl+E
Wait+Screen /Not run: the environment "prod" is read-only/
Sleep 3s
