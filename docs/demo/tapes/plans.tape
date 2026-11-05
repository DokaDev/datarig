# A query's plan as a tree, with times, then its summary.
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
# Each line ends before Enter with no completion open (a trailing space closes it).
Type "SELECT p.category, count(*) AS items, sum(i.quantity * i.unit_price) AS revenue "
Enter
Type "FROM store.order_items i "
Enter
Type "JOIN store.products p ON p.id = i.product_id "
Enter
Type "JOIN store.orders o ON o.id = i.order_id "
Enter
Type "WHERE o.placed_at >= '2026-06-01' "
Enter
Type "GROUP BY p.category ORDER BY revenue DESC "
Escape
Sleep 200ms
Escape
Wait+Screen /NORMAL/
Sleep 800ms

# EXPLAIN ANALYZE of the statement under the cursor.
Space
Type "ea"
Wait+Screen /Plan · Tree/
Wait+Screen /Hash Join/
Sleep 3s
Screenshot plans.png
Tab
Sleep 300ms
Type "2"
Wait+Screen /Slowest node/
Sleep 3s
