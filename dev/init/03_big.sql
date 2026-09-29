-- Big table: 4,000,000 rows for streaming / row-limit / cancel tests.
-- Deliberately has NO index on event_type or created_at ordering helpers other than PK,
-- so `ORDER BY created_at DESC` forces a full sort (shows why row limits != less server work).
INSERT INTO analytics.events (id, user_id, event_type, page, payload, created_at)
SELECT g,
       CASE WHEN g % 10 = 0 THEN NULL ELSE 1 + (g * 2654435761 % 5000) END,
       (ARRAY['page_view','click','search','add_to_cart','purchase','ログイン','会員登録'])[1 + g % 7],
       (ARRAY['/','/products','/cart','/checkout','/検索?q=イヤホン','/mypage'])[1 + (g / 7) % 6],
       CASE WHEN g % 4 = 0 THEN NULL ELSE jsonb_build_object('session', g / 20, 'ms', g % 1500) END,
       timestamptz '2025-01-01 00:00:00+09' + g * interval '7 seconds'
FROM generate_series(1, 4000000) g;

ANALYZE;
