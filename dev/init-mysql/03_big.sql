-- Moderately big table (~1,000,000 rows) for streaming / row-limit tests.
-- Deliberately has no index beyond the PK, so ORDER BY created_at forces a full sort.
-- Built from a 100x100x100 cross join (recursion depth 100, well under the default
-- cte_max_recursion_depth of 1000) instead of row-by-row inserts, so this stays fast.

USE shop;

INSERT INTO events (id, user_id, event_type, page, payload, created_at)
WITH RECURSIVE digits AS (
    SELECT 0 AS i
    UNION ALL
    SELECT i + 1 FROM digits WHERE i < 99
)
SELECT g.n,
       CASE WHEN g.n % 10 = 0 THEN NULL ELSE 1 + (g.n * 2654435761 MOD 5000) END,
       ELT(1 + (g.n MOD 7), 'page_view','click','search','add_to_cart','purchase','ログイン','会員登録'),
       ELT(1 + ((g.n DIV 7) MOD 6), '/','/products','/cart','/checkout','/検索?q=イヤホン','/mypage'),
       CASE WHEN g.n % 4 = 0 THEN NULL ELSE JSON_OBJECT('session', g.n DIV 20, 'ms', g.n MOD 1500) END,
       TIMESTAMP('2025-01-01 00:00:00') + INTERVAL g.n SECOND
FROM (
    SELECT a.i + b.i * 100 + c.i * 10000 + 1 AS n
    FROM digits a, digits b, digits c
) g;

ANALYZE TABLE events;
