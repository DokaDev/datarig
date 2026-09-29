-- Seed data: CJK + mixed-width text, emoji, NULLs, long text, jsonb, numeric
SELECT setseed(0.42);

-- ── handcrafted edge-case users (ids 1..8) ──────────────────────────
INSERT INTO shop.users (email, name, nickname, address, phone, bio, profile, is_active, created_at) VALUES
('kim@example.com',   '陳大文',        '大文🐘',          '臺北市信義區信義路五段7號, 台北101大樓 10樓', '010-1234-5678', '你好。我是一位喜歡資料庫的開發者。', '{"lang":"zh-TW","tags":["dev","db"]}', true,  '2024-01-02 09:00:00+09'),
('lee@example.com',   '林美玲',        NULL,              '高雄市前鎮區成功二路 55號',               NULL,            NULL,                                               '{}',                                true,  '2024-02-14 18:30:00+09'),
('park@example.com',  '張志明',        '👨‍👩‍👧‍👦 family',    NULL,                                            '010-0000-0000', repeat('很長的自我介紹。Long bio text mixed with 中文. ', 200), '{"vip":true,"level":7}', false, '2024-03-01 00:00:00+09'),
('emoji@example.com', 'Emoji テスト',   '🔥🚀✨🎉',          '沖縄県那覇市おもろまち 242 🏝️',              '+81-90-9999-8888', '絵文字の幅テスト 🇯🇵🇹🇼🇺🇸 ✅❌⚠️', '{"flags":["🇯🇵","🇺🇸"]}', true, '2024-04-05 12:34:56+09'),
('jp@example.com',    '山田太郎',       'やまだ',            '東京都渋谷区道玄坂1-2-3',                         NULL,            '日本語と中文とEnglishが混在するテキスト',            '{"lang":"ja"}',                     true,  '2024-05-06 07:08:09+09'),
('zh@example.com',    '王小明',         NULL,              '北京市海淀区中关村大街1号',                        NULL,            '中文测试文本',                                       '{"lang":"zh"}',                     true,  '2024-06-07 10:11:12+09'),
('ascii@example.com', 'Plain ASCII User', 'ascii_only',     '1600 Amphitheatre Pkwy, Mountain View, CA',       '+1-650-000-0000', 'Only ASCII here.',                                 '{"lang":"en"}',                     true,  '2024-07-08 13:14:15+09'),
('combo@example.com', '全角 ＡＢＣ 半角 ｱｲｳ', 'é combining', E'タブ\t入り住所 (tab inside)',                     NULL,            E'複数行\nテキスト\n三行目',                          '{"multiline":true}',                true,  '2024-08-09 16:17:18+09');

-- ── generated users (≈5,000) ────────────────────────────────────────
WITH fam AS (SELECT ARRAY['王','李','張','劉','陳','楊','黃','趙','吳','周','徐','孫','馬','朱','胡','郭','何','高','林','羅'] a),
     giv AS (SELECT ARRAY['子涵','欣怡','梓萱','思彤','浩然','宇軒','雨桐','俊傑','詩涵','家豪','嘉欣','志明','淑芬','建國','美玲','文傑','雅婷','冠宇','佳穎','承恩'] a),
     city AS (SELECT ARRAY['東京都渋谷区','東京都新宿区','大阪府大阪市北区','神奈川県横浜市西区','愛知県名古屋市中区','福岡県福岡市博多区','北海道札幌市中央区','京都府京都市下京区','兵庫県神戸市中央区','宮城県仙台市青葉区'] a)
INSERT INTO shop.users (email, name, nickname, address, phone, bio, profile, is_active, created_at)
SELECT 'user' || g || '@example.com',
       fam.a[1 + (g % 20)] || giv.a[1 + ((g / 20) % 20)],
       CASE WHEN g % 7 = 0 THEN NULL WHEN g % 11 = 0 THEN 'ニック' || g || ' 😀' ELSE 'nick' || g END,
       CASE WHEN g % 13 = 0 THEN NULL ELSE city.a[1 + (g % 10)] || ' ' || (g % 300 + 1) || '丁目 ' || (g % 50 + 1) END,
       CASE WHEN g % 5 = 0 THEN NULL ELSE '010-' || lpad((g % 10000)::text, 4, '0') || '-' || lpad(((g * 7) % 10000)::text, 4, '0') END,
       CASE WHEN g % 97 = 0 THEN repeat('とても長い自己紹介 ', 150) WHEN g % 3 = 0 THEN NULL ELSE '会員 ' || g || ' の紹介' END,
       jsonb_build_object('grade', (ARRAY['bronze','silver','gold','platinum'])[1 + g % 4], 'points', g * 13 % 10000, 'marketing', g % 2 = 0),
       g % 17 <> 0,
       timestamptz '2023-01-01 00:00:00+09' + (g * interval '97 minutes')
FROM generate_series(1, 4992) g, fam, giv, city;

-- ── products (500) ──────────────────────────────────────────────────
WITH cat AS (SELECT ARRAY['家電','書籍','衣料','食品','家具','運動','美容','玩具'] a),
     adj AS (SELECT ARRAY['高級','超軽量','環境配慮','スマート','クラシック','ミニ','大容量','限定版'] a),
     noun AS (SELECT ARRAY['無線イヤホン','タンブラー','ノートPCスタンド','ランニングシューズ','ブルートゥーススピーカー','キャンプチェア','ハンドクリーム','ボードゲーム','メカニカルキーボード','コーヒー豆'] a)
INSERT INTO shop.products (sku, name, category, price, stock, description, attributes, created_at)
SELECT 'SKU-' || lpad(g::text, 6, '0'),
       adj.a[1 + g % 8] || ' ' || noun.a[1 + (g / 8) % 10] || CASE WHEN g % 25 = 0 THEN ' 🎁' ELSE '' END,
       cat.a[1 + g % 8],
       round((1000 + random() * 499000)::numeric, -1),
       (random() * 1000)::int,
       CASE WHEN g % 4 = 0 THEN NULL ELSE '商品 ' || g || ' の説明。Description for product ' || g || '.' END,
       CASE WHEN g % 3 = 0 THEN NULL ELSE jsonb_build_object('color', (ARRAY['black','white','赤','青'])[1 + g % 4], 'weight_g', g * 3 % 2000) END,
       timestamptz '2023-06-01 00:00:00+09' + (g * interval '1 day') / 3
FROM generate_series(1, 500) g, cat, adj, noun;

-- ── orders (50,000) ─────────────────────────────────────────────────
INSERT INTO shop.orders (user_id, status, total_amount, memo, shipping, ordered_at)
SELECT 1 + (random() * 4999)::int,
       (ARRAY['pending','paid','shipped','cancelled'])[1 + (random() * 3)::int],
       0,
       CASE WHEN g % 9 = 0 THEN '玄関前に置いてください 🙏' WHEN g % 10 = 0 THEN 'Leave at the door' ELSE NULL END,
       jsonb_build_object('method', (ARRAY['宅配','バイク便','コンビニ受取'])[1 + g % 3], 'fee', (g % 3) * 1500),
       timestamptz '2024-01-01 00:00:00+09' + random() * interval '600 days'
FROM generate_series(1, 50000) g;

-- ── order_items (1..5 lines per order) ──────────────────────────────
INSERT INTO shop.order_items (order_id, line_no, product_id, quantity, unit_price)
SELECT o.id, l, p.id, 1 + (random() * 4)::int, p.price
FROM shop.orders o
CROSS JOIN LATERAL generate_series(1, 1 + (o.id % 5)::int) l
JOIN shop.products p ON p.id = 1 + ((o.id * 31 + l * 17) % 500);

UPDATE shop.orders o
SET total_amount = s.total
FROM (SELECT order_id, sum(quantity * unit_price) AS total FROM shop.order_items GROUP BY order_id) s
WHERE s.order_id = o.id;

-- ── reviews (unique per product/user) ───────────────────────────────
INSERT INTO shop.reviews (product_id, user_id, rating, title, body, created_at)
SELECT DISTINCT ON (pid, uid) pid, uid,
       1 + (random() * 4)::int,
       CASE WHEN g % 6 = 0 THEN NULL ELSE (ARRAY['最高です 👍','まあまあ','配送が早い 🚚','また買いたい','いまいち 😞'])[1 + g % 5] END,
       CASE WHEN g % 50 = 0 THEN repeat('本当に満足できる製品です。', 80) ELSE 'レビュー本文 ' || g END,
       timestamptz '2024-02-01 00:00:00+09' + (g * interval '13 minutes')
FROM (SELECT g, 1 + (g * 7) % 500 AS pid, 1 + (g * 13) % 5000 AS uid FROM generate_series(1, 20000) g) s;

-- ── audit_log (no PK) ───────────────────────────────────────────────
INSERT INTO shop.audit_log (occurred_at, actor, action, detail)
SELECT timestamptz '2024-01-01 00:00:00+09' + g * interval '1 hour',
       CASE WHEN g % 4 = 0 THEN NULL ELSE 'admin' || (g % 3) END,
       (ARRAY['login','update_price','refund','export'])[1 + g % 4],
       jsonb_build_object('ip', '10.0.' || (g % 255) || '.' || (g * 7 % 255), 'note', CASE WHEN g % 2 = 0 THEN '管理者作業' ELSE NULL END)
FROM generate_series(1, 1000) g;
-- deliberate duplicate rows (identical values) — why no-PK tables must be read-only
INSERT INTO shop.audit_log SELECT * FROM shop.audit_log ORDER BY occurred_at LIMIT 5;

-- ── analytics.daily_stats ───────────────────────────────────────────
INSERT INTO analytics.daily_stats (stat_date, metric, value)
SELECT d::date, m, round((random() * 100000)::numeric, 2)
FROM generate_series(date '2024-01-01', date '2025-12-31', interval '1 day') d,
     unnest(ARRAY['dau','revenue','signups','転換率']) m;

-- ── Korean rows ─────────────────────────────────────────────────────
-- Korean text is written as U&'' escapes (\XXXX is a code point) that the server decodes
-- when it loads the file, so no Hangul appears in the repository; the English comment above
-- each row says what it reads. U&'' needs standard_conforming_strings, the default since 9.1.
SET standard_conforming_strings = on;

INSERT INTO shop.users (email, name, nickname, address, phone, bio, profile, is_active, created_at) VALUES
-- Korean: Hong Gil-dong; nickname "Gil-dong"; Sejong-daero, Jongno-gu, Seoul; bio "Hello. I am a developer who likes databases."
('hong@example.com', U&'\D64D\AE38\B3D9', U&'\AE38\B3D9\C774', U&'\C11C\C6B8\D2B9\BCC4\C2DC \C885\B85C\AD6C \C138\C885\B300\B85C 175', '010-2024-1009', U&'\C548\B155\D558\C138\C694. \B370\C774\D130\BCA0\C774\C2A4\B97C \C88B\C544\D558\B294 \AC1C\BC1C\C790\C785\B2C8\B2E4.', jsonb_build_object('lang', 'ko', 'city', U&'\C11C\C6B8'), true, '2024-09-10 09:00:00+09'),
-- Korean: Kim Min-ji; nickname mixes Latin, emoji and Hangul; Haeundae, Busan; very long Korean bio
('minji@example.com', U&'\AE40\BBFC\C9C0', U&'minji ✨ \BBFC\C9C0', U&'\BD80\C0B0\AD11\C5ED\C2DC \D574\C6B4\B300\AD6C \D574\C6B4\B300\D574\BCC0\B85C 264', NULL, repeat(U&'\C544\C8FC \AE34 \C790\AE30\C18C\AC1C\C785\B2C8\B2E4. \D55C\AD6D\C5B4 \BB38\C7A5\C774 \ACC4\C18D \C774\C5B4\C9D1\B2C8\B2E4. ', 120), jsonb_build_object('lang', 'ko', 'city', U&'\BD80\C0B0'), true, '2024-10-11 10:11:12+09'),
-- Korean: Lee Seo-jun; Jeju address with emoji; bio mixes Korean, English, Japanese, Chinese and a flag
('seojun@example.com', U&'\C774\C11C\C900', NULL, U&'\C81C\C8FC\D2B9\BCC4\C790\CE58\B3C4 \C81C\C8FC\C2DC \CCA8\B2E8\B85C 242 🏝️', '+82-10-5555-0242', U&'\D55C\AD6D\C5B4 English 日本語 中文 🇰🇷 \C11E\C778 \D14D\C2A4\D2B8 ✅', jsonb_build_object('lang', 'ko', 'city', U&'\C81C\C8FC'), false, '2024-11-12 13:14:15+09'),
-- Korean: name is "Hangul" in decomposed jamo (NFD, 6 code points), nickname uses compatibility jamo (laughter), bio says "The name of this row is in NFD (decomposed jamo) form."
('nfd@example.com', U&'\1112\1161\11AB\1100\1173\11AF NFD', U&'\314B\314B\314B 😂', NULL, NULL, U&'\C774 \D589\C758 \C774\B984\C740 NFD(\C790\BAA8 \BD84\D574) \D615\D0DC\C785\B2C8\B2E4.', '{"lang":"ko"}', true, '2024-12-13 16:17:18+09');

INSERT INTO shop.products (sku, name, category, price, stock, description, attributes, created_at) VALUES
-- Korean: Wireless Earbuds Pro, home appliances; "Noise cancelling. Bluetooth 5.3, up to 30 hours of playback"
('SKU-KR-0001', U&'\BB34\C120 \C774\C5B4\D3F0 \D504\B85C', U&'\AC00\C804', 189000.00, 120, U&'\B178\C774\C988 \CE94\C2AC\B9C1 \C9C0\C6D0. Bluetooth 5.3, \CD5C\B300 30\C2DC\AC04 \C7AC\C0DD 🎧', jsonb_build_object('color', 'black', 'origin', 'KR'), '2024-09-01 00:00:00+09'),
-- Korean: Jeju tangerines 5kg, food; out of stock, no description
('SKU-KR-0002', U&'\C81C\C8FC \AC10\ADE4 5kg 🍊', U&'\C2DD\D488', 32900.00, 0, NULL, NULL, '2024-09-02 00:00:00+09'),
-- Korean: Solid wood desk, furniture; very long description; color "walnut"
('SKU-KR-0003', U&'\C6D0\BAA9 \CC45\C0C1 (1200×600)', U&'\AC00\AD6C', 459000.00, 7, repeat(U&'\D2BC\D2BC\D55C \C6D0\BAA9\C73C\B85C \B9CC\B4E0 \CC45\C0C1\C785\B2C8\B2E4. ', 60), jsonb_build_object('color', U&'\C6D4\B11B', 'origin', 'KR'), '2024-09-03 00:00:00+09'),
-- Korean: Limited edition board game "Hangul Play", toys; "2-6 players, ages 8+. 128 jamo cards included"
('SKU-KR-0004', U&'\D55C\C815\D310 \BCF4\B4DC\AC8C\C784 「\D55C\AE00 \B180\C774」', U&'\C644\AD6C', 45000.00, 33, U&'2~6\C778\C6A9, 8\C138 \C774\C0C1. \D55C\AE00 \C790\BAA8 \CE74\B4DC 128\C7A5 \D3EC\D568', jsonb_build_object('color', 'white', 'origin', 'KR'), '2024-09-04 00:00:00+09');

INSERT INTO shop.reviews (product_id, user_id, rating, title, body, created_at)
-- Korean: title "The best", body "Great sound and long battery life. Highly recommended!"
SELECT p.id, u.id, 5, U&'\CD5C\ACE0\C608\C694 👍', U&'\C74C\C9C8\C774 \C815\B9D0 \C88B\ACE0 \BC30\D130\B9AC\B3C4 \C624\B798\AC00\C694. \AC15\B825 \CD94\CC9C\D569\B2C8\B2E4!', timestamptz '2024-09-20 20:00:00+09'
FROM shop.products p, shop.users u WHERE p.sku = 'SKU-KR-0001' AND u.email = 'hong@example.com'
UNION ALL
-- Korean: title "So-so", very long body "It was a little uncomfortable to wear."
SELECT p.id, u.id, 2, U&'\ADF8\C800 \ADF8\B798\C694', repeat(U&'\CC29\C6A9\AC10\C774 \C870\AE08 \BD88\D3B8\D588\C5B4\C694. ', 80), timestamptz '2024-10-20 21:00:00+09'
FROM shop.products p, shop.users u WHERE p.sku = 'SKU-KR-0001' AND u.email = 'minji@example.com'
UNION ALL
-- Korean: no title, body mixes English and Korean: "Delivery was fast but assembly is hard"
SELECT p.id, u.id, 4, NULL, U&'Delivery \BE68\B790\C5B4\C694 🚚 but \C870\B9BD\C774 \C5B4\B824\C6C0', timestamptz '2024-11-20 22:00:00+09'
FROM shop.products p, shop.users u WHERE p.sku = 'SKU-KR-0003' AND u.email = 'seojun@example.com'
UNION ALL
-- Korean: title "Fast delivery", no body
SELECT p.id, u.id, 3, U&'\BC30\C1A1\C774 \BE68\B77C\C694 🚚', NULL, timestamptz '2024-12-20 23:00:00+09'
FROM shop.products p, shop.users u WHERE p.sku = 'SKU-KR-0004' AND u.email = 'nfd@example.com';

-- Korean: memo "Please leave it at the door", shipping method "parcel delivery"
INSERT INTO shop.orders (user_id, status, total_amount, memo, shipping, ordered_at)
SELECT id, 'paid', 0, U&'\BB38 \C55E\C5D0 \B193\C544 \C8FC\C138\C694 🙏', jsonb_build_object('method', U&'\D0DD\BC30', 'fee', 3000), '2024-09-15 12:00:00+09' FROM shop.users WHERE email = 'hong@example.com';
INSERT INTO shop.order_items (order_id, line_no, product_id, quantity, unit_price)
SELECT o.id, l.line_no, p.id, l.qty, p.price
FROM shop.orders o JOIN shop.users u ON u.id = o.user_id AND u.email = 'hong@example.com',
     (SELECT 1 AS line_no, 'SKU-KR-0001' AS sku, 1 AS qty UNION ALL SELECT 2, 'SKU-KR-0004', 2) l
JOIN shop.products p ON p.sku = l.sku;
UPDATE shop.orders o SET total_amount = s.total
FROM (SELECT order_id, sum(quantity * unit_price) AS total FROM shop.order_items GROUP BY order_id) s
WHERE s.order_id = o.id AND o.user_id = (SELECT id FROM shop.users WHERE email = 'hong@example.com');

-- Korean: note "administrator work"
INSERT INTO shop.audit_log (occurred_at, actor, action, detail)
VALUES ('2024-09-30 18:00:00+09', 'admin_kr', 'export', jsonb_build_object('ip', '10.0.82.1', 'note', U&'\AD00\B9AC\C790 \C791\C5C5'));
