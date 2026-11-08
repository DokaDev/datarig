#!/bin/sh
# The load of the monitor recording (docs/demo/tapes/monitor.tape), so its charts move and its
# Sessions and Locks views have something to show. The demo-load service of docs/demo/compose.yml
# runs it, and render.sh starts that service just before the tape and removes it right after;
# it reaches the demo PostgreSQL only (by its service name on the demo project's own network).
# Every session it opens is a client in that container, so removing the service ends them all.
# The same statements run at the same pace on every render, and nothing in the demo data
# changes: the writes go to temporary tables, and the one row lock is taken by a SELECT ... FOR
# UPDATE.
#   checkout (2 sessions)  short reads, and writes to a temporary table, at a pace that rises
#                          and falls over 20 seconds
#   reports                a short report every few seconds
#   analytics              one long report for as long as the load runs
#   payments               holds the lock on order 1042 while it sleeps: the blocker
#   fulfilment             waits for that lock: the waiter
# The blocker and the waiter run once: when the recording cancels the blocker's statement, the
# waiter takes the lock and its statement ends, and nobody waits any more. Their sessions stay,
# idle.
set -eu

# A session whose client is gone ends at once, even in the middle of pg_sleep.
export PGOPTIONS='-c client_connection_check_interval=1000'

client() {
  PGAPPNAME=$1 psql -qX -o /dev/null
}

# A client that runs one statement and then stays connected, idle.
once() {
  { echo "$2"; sleep 3600; } | client "$1"
}

# Two statements (a read and a write) per step, then the step's pause.
checkout() {
  id=$1
  echo "CREATE TEMPORARY TABLE cart_events (order_id integer, at timestamptz DEFAULT now());"
  while :; do
    # Each pace (pause:steps) lasts about 2 seconds.
    for pace in 0.02:100 0.03:66 0.05:40 0.1:20 0.25:8 0.5:4 0.25:8 0.1:20 0.05:40 0.03:66; do
      n=${pace#*:}
      while [ "$n" -gt 0 ]; do
        id=$((id % 20000 + 1))
        echo "SELECT status, total FROM store.orders WHERE id = $id;"
        echo "INSERT INTO cart_events (order_id) VALUES ($id);"
        sleep "${pace%:*}"
        n=$((n - 1))
      done
    done
  done
}

reports() {
  while :; do
    cat <<'SQL'
WITH monthly AS (
    SELECT date_trunc('month', o.placed_at)::date AS month, p.category, o.channel,
           sum(i.quantity * i.unit_price) AS revenue, count(DISTINCT o.id) AS orders
    FROM store.order_items i
    JOIN store.orders o ON o.id = i.order_id
    JOIN store.products p ON p.id = i.product_id
    WHERE o.status <> 'cancelled'
    GROUP BY 1, 2, 3
)
SELECT month, category, channel, revenue, orders,
       round(revenue / nullif(sum(revenue) OVER (PARTITION BY month), 0), 3) AS share
FROM monthly, pg_sleep(2)
ORDER BY month, revenue DESC;
SQL
    sleep 3
  done
}

# Whether a session of this application is in the given wait (one psql of its own).
waiting() {
  [ "$(PGAPPNAME=demo-load psql -qXtA -c "SELECT count(*) FROM pg_stat_activity
      WHERE application_name = '$1' AND wait_event = '$2'")" = 1 ]
}

once analytics "$(cat <<'SQL'
WITH first_orders AS (
    SELECT customer_id, min(placed_at) AS first_at
    FROM store.orders
    GROUP BY customer_id
), lifetime AS (
    SELECT c.id, c.tier, c.city, count(o.id) AS orders, sum(o.total) AS spent,
           max(o.placed_at) - f.first_at AS active_for
    FROM store.customers c
    JOIN store.orders o ON o.customer_id = c.id
    JOIN first_orders f ON f.customer_id = c.id
    WHERE o.status IN ('paid', 'shipped', 'delivered')
    GROUP BY c.id, c.tier, c.city, f.first_at
)
SELECT tier, city, count(*) AS customers, round(avg(spent), 2) AS avg_spent,
       round(avg(orders), 1) AS avg_orders, avg(active_for) AS avg_active_for
FROM lifetime, pg_sleep(600)
GROUP BY tier, city
ORDER BY avg_spent DESC
LIMIT 20;
SQL
)" &
until waiting analytics PgSleep; do sleep 0.2; done
checkout 1 | client checkout &
checkout 10001 | client checkout &
reports | client reports &

# The blocker first, and the waiter only once the blocker holds the row.
once payments "WITH held AS (SELECT id FROM store.orders WHERE id = 1042 FOR UPDATE)
SELECT pg_sleep(600) FROM held;" &
until waiting payments PgSleep; do sleep 0.2; done
once fulfilment "SELECT id, status FROM store.orders WHERE id = 1042 FOR UPDATE;" &
until waiting fulfilment transactionid; do sleep 0.2; done
echo "demo load running"

wait
