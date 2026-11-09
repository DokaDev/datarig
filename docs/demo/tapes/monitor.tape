# The server monitor on a server with work to show (docs/demo/load/monitor.sh, which render.sh
# runs only while this tape records): the Overview's charts, the running sessions, one
# session's whole statement, a session waiting for another's row lock in the Locks view, and
# the guarded cancel of the blocker's statement, after which nobody waits.
Require datarig
Source settings.tape

# The explorer is hidden first, so the tab has the whole width. The tab opens hidden and reads
# every second (the demo configuration's interval) for a while, so its charts are full.
Hide
Type "datarig acme-staging"
Enter
Wait+Screen /Connected to acme-staging/
Space
Type "b"
Wait+Screen /(?m)^╭ Query · console 1/
Type ":monitor"
Enter
Wait+Screen /Monitor: acme-staging/
Wait+Screen /every 1s/
Wait+Screen /3 Locks \(1\)/
Sleep 40s
Show

Wait+Screen /active \d+ · waiting 1/
Sleep 4s

# The sessions, the longest running statement first: the analytics report's, in full.
Type "2"
Wait+Screen /\d+ of \d+ sessions · \d+ idle not listed/
Sleep 1.5s
Enter
Wait+Screen /application\s+analytics/
Sleep 1.2s
Type@80ms "jjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjj"
Wait+Screen /\bLIMIT\b/
Sleep 2s
Escape

# Who waits for whom: fulfilment waits for the row payments holds.
Type "3"
Wait+Screen /1 waiting for a lock/
Wait+Screen /▸ \d+ demo · active .* holds ExclusiveLock on transactionid/
Wait+Screen /└ \d+ demo · waits .* for ShareLock on transactionid/
Sleep 2.5s
Screenshot monitor.png
Sleep 300ms

# The blocker in full, and its statement cancelled: only `y` sends the cancel.
Enter
Wait+Screen /application\s+payments/
Sleep 1.5s
Type "c"
Wait+Screen /Cancel the statement of session \d+ \(demo, acme, /
Sleep 2s
Type "y"
Wait+Screen /The statement of session \d+ ended after the cancel was sent/
Sleep 1.5s
Escape
Type "3"
Wait+Screen /No session waits for a lock/
Sleep 2s

# The action list: what was sent and what the reads after it showed.
Type "2"
Sleep 300ms
Type "L"
Wait+Screen /cancel \d+\s+demo@acme/
Sleep 3s
