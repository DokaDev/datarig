-- Seed data: CJK + mixed-width text, emoji, NULLs, long text, JSON, numeric
-- Comparable to dev/init/02_seed.sql (PostgreSQL) but scaled down for MySQL.

USE shop;

-- Recursive CTEs below go past the default depth of 1000.
SET SESSION cte_max_recursion_depth = 20000;

-- ── handcrafted edge-case users (ids 1..8) ──────────────────────────
INSERT INTO users (email, name, nickname, address, phone, bio, profile, is_active, created_at) VALUES
('kim@example.com',   '陳大文',        '大文🐘',          '臺北市信義區信義路五段7號, 台北101大樓 10樓', '010-1234-5678', '你好。我是一位喜歡資料庫的開發者。', JSON_OBJECT('lang', 'zh-TW', 'tags', JSON_ARRAY('dev', 'db')), 1, '2024-01-02 09:00:00'),
('lee@example.com',   '林美玲',        NULL,              '高雄市前鎮區成功二路 55號',               NULL,            NULL,                                               JSON_OBJECT(),                                       1, '2024-02-14 18:30:00'),
('park@example.com',  '張志明',        '👨‍👩‍👧‍👦 family',    NULL,                                            '010-0000-0000', REPEAT('很長的自我介紹。Long bio text mixed with 中文. ', 200), JSON_OBJECT('vip', TRUE, 'level', 7), 0, '2024-03-01 00:00:00'),
('emoji@example.com', 'Emoji テスト',   '🔥🚀✨🎉',          '沖縄県那覇市おもろまち 242 🏝️',              '+81-90-9999-8888', '絵文字の幅テスト 🇯🇵🇹🇼🇺🇸 ✅❌⚠️', JSON_OBJECT('flags', JSON_ARRAY('🇯🇵', '🇺🇸')), 1, '2024-04-05 12:34:56'),
('jp@example.com',    '山田太郎',       'やまだ',            '東京都渋谷区道玄坂1-2-3',                         NULL,            '日本語と中文とEnglishが混在するテキスト',            JSON_OBJECT('lang', 'ja'),                          1, '2024-05-06 07:08:09'),
('zh@example.com',    '王小明',         NULL,              '北京市海淀区中关村大街1号',                        NULL,            '中文测试文本',                                       JSON_OBJECT('lang', 'zh'),                          1, '2024-06-07 10:11:12'),
('ascii@example.com', 'Plain ASCII User', 'ascii_only',     '1600 Amphitheatre Pkwy, Mountain View, CA',       '+1-650-000-0000', 'Only ASCII here.',                                 JSON_OBJECT('lang', 'en'),                          1, '2024-07-08 13:14:15'),
('combo@example.com', '全角 ＡＢＣ 半角 ｱｲｳ', 'é combining', 'タブ\t入り住所 (tab inside)',                     NULL,            '複数行\nテキスト\n三行目',                          JSON_OBJECT('multiline', TRUE),                     1, '2024-08-09 16:17:18');

-- ── generated users (≈2,000) ────────────────────────────────────────
INSERT INTO users (email, name, nickname, address, phone, bio, profile, is_active, created_at)
WITH RECURSIVE seq AS (
    SELECT 1 AS g
    UNION ALL
    SELECT g + 1 FROM seq WHERE g < 2000
)
SELECT CONCAT('user', g, '@example.com'),
       CONCAT(
           ELT(1 + (g % 20), '王','李','張','劉','陳','楊','黃','趙','吳','周','徐','孫','馬','朱','胡','郭','何','高','林','羅'),
           ELT(1 + ((g DIV 20) % 20), '子涵','欣怡','梓萱','思彤','浩然','宇軒','雨桐','俊傑','詩涵','家豪','嘉欣','志明','淑芬','建國','美玲','文傑','雅婷','冠宇','佳穎','承恩')
       ),
       CASE WHEN g % 7 = 0 THEN NULL WHEN g % 11 = 0 THEN CONCAT('ニック', g, ' 😀') ELSE CONCAT('nick', g) END,
       CASE WHEN g % 13 = 0 THEN NULL ELSE CONCAT(
           ELT(1 + (g % 10), '東京都渋谷区','東京都新宿区','大阪府大阪市北区','神奈川県横浜市西区','愛知県名古屋市中区','福岡県福岡市博多区','北海道札幌市中央区','京都府京都市下京区','兵庫県神戸市中央区','宮城県仙台市青葉区'),
           ' ', (g % 300 + 1), '丁目 ', (g % 50 + 1)
       ) END,
       CASE WHEN g % 5 = 0 THEN NULL ELSE CONCAT('010-', LPAD(g % 10000, 4, '0'), '-', LPAD((g * 7) % 10000, 4, '0')) END,
       CASE WHEN g % 97 = 0 THEN REPEAT('とても長い自己紹介 ', 150) WHEN g % 3 = 0 THEN NULL ELSE CONCAT('会員 ', g, ' の紹介') END,
       JSON_OBJECT('grade', ELT(1 + g % 4, 'bronze','silver','gold','platinum'), 'points', g * 13 % 10000, 'marketing', g % 2 = 0),
       g % 17 <> 0,
       TIMESTAMP('2023-01-01 00:00:00') + INTERVAL (g * 97) MINUTE
FROM seq;

-- ── products (300) ──────────────────────────────────────────────────
INSERT INTO products (sku, name, category, price, stock, description, attributes, created_at)
WITH RECURSIVE seq AS (
    SELECT 1 AS g
    UNION ALL
    SELECT g + 1 FROM seq WHERE g < 300
)
SELECT CONCAT('SKU-', LPAD(g, 6, '0')),
       CONCAT(
           ELT(1 + g % 8, '高級','超軽量','環境配慮','スマート','クラシック','ミニ','大容量','限定版'),
           ' ',
           ELT(1 + (g DIV 8) % 10, '無線イヤホン','タンブラー','ノートPCスタンド','ランニングシューズ','ブルートゥーススピーカー','キャンプチェア','ハンドクリーム','ボードゲーム','メカニカルキーボード','コーヒー豆'),
           CASE WHEN g % 25 = 0 THEN ' 🎁' ELSE '' END
       ),
       ELT(1 + g % 8, '家電','書籍','衣料','食品','家具','運動','美容','玩具'),
       ROUND(1000 + RAND() * 499000, -1),
       FLOOR(RAND() * 1000),
       CASE WHEN g % 4 = 0 THEN NULL ELSE CONCAT('商品 ', g, ' の説明。Description for product ', g, '.') END,
       CASE WHEN g % 3 = 0 THEN NULL ELSE JSON_OBJECT('color', ELT(1 + g % 4, 'black','white','赤','青'), 'weight_g', g * 3 % 2000) END,
       TIMESTAMP('2023-06-01 00:00:00') + INTERVAL FLOOR(g * 8) HOUR
FROM seq;

-- ── orders (10,000) ─────────────────────────────────────────────────
INSERT INTO orders (user_id, status, total_amount, memo, shipping, ordered_at)
WITH RECURSIVE seq AS (
    SELECT 1 AS g
    UNION ALL
    SELECT g + 1 FROM seq WHERE g < 10000
)
SELECT 1 + FLOOR(RAND() * 2007),
       ELT(1 + FLOOR(RAND() * 4), 'pending','paid','shipped','cancelled'),
       0,
       CASE WHEN g % 9 = 0 THEN '玄関前に置いてください 🙏' WHEN g % 10 = 0 THEN 'Leave at the door' ELSE NULL END,
       JSON_OBJECT('method', ELT(1 + g % 3, '宅配','バイク便','コンビニ受取'), 'fee', (g % 3) * 1500),
       TIMESTAMP('2024-01-01 00:00:00') + INTERVAL FLOOR(RAND() * 600 * 24 * 60) MINUTE
FROM seq;

-- ── order_items (1..5 lines per order) ──────────────────────────────
INSERT INTO order_items (order_id, line_no, product_id, quantity, unit_price)
SELECT o.id, l.line_no, 1 + ((o.id * 31 + l.line_no * 17) % 300), 1 + FLOOR(RAND() * 4), p.price
FROM orders o
JOIN (
    SELECT 1 AS line_no UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 UNION ALL SELECT 5
) l ON l.line_no <= 1 + (o.id % 5)
JOIN products p ON p.id = 1 + ((o.id * 31 + l.line_no * 17) % 300);

UPDATE orders o
JOIN (SELECT order_id, SUM(quantity * unit_price) AS total FROM order_items GROUP BY order_id) s
    ON s.order_id = o.id
SET o.total_amount = s.total;

-- ── reviews (unique per product/user) ───────────────────────────────
INSERT INTO reviews (product_id, user_id, rating, title, body, created_at)
WITH RECURSIVE seq AS (
    SELECT 1 AS g
    UNION ALL
    SELECT g + 1 FROM seq WHERE g < 8000
)
SELECT pid, uid, rating, title, body, created_at
FROM (
    SELECT g,
           1 + (g * 7) % 300 AS pid,
           1 + (g * 13) % 2008 AS uid,
           1 + FLOOR(RAND() * 4) AS rating,
           CASE WHEN g % 6 = 0 THEN NULL ELSE ELT(1 + g % 5, '最高です 👍','まあまあ','配送が早い 🚚','また買いたい','いまいち 😞') END AS title,
           CASE WHEN g % 50 = 0 THEN REPEAT('本当に満足できる製品です。', 80) ELSE CONCAT('レビュー本文 ', g) END AS body,
           TIMESTAMP('2024-02-01 00:00:00') + INTERVAL (g * 13) MINUTE AS created_at,
           ROW_NUMBER() OVER (PARTITION BY 1 + (g * 7) % 300, 1 + (g * 13) % 2008 ORDER BY g) AS rn
    FROM seq
) dedup
WHERE rn = 1;

-- ── audit_log (no PK) ───────────────────────────────────────────────
INSERT INTO audit_log (occurred_at, actor, action, detail)
WITH RECURSIVE seq AS (
    SELECT 1 AS g
    UNION ALL
    SELECT g + 1 FROM seq WHERE g < 1000
)
SELECT TIMESTAMP('2024-01-01 00:00:00') + INTERVAL g HOUR,
       CASE WHEN g % 4 = 0 THEN NULL ELSE CONCAT('admin', g % 3) END,
       ELT(1 + g % 4, 'login','update_price','refund','export'),
       JSON_OBJECT('ip', CONCAT('10.0.', g % 255, '.', g * 7 % 255), 'note', CASE WHEN g % 2 = 0 THEN '管理者作業' ELSE NULL END)
FROM seq;
-- deliberate duplicate rows (identical values) — why no-PK tables must be read-only
INSERT INTO audit_log SELECT * FROM audit_log ORDER BY occurred_at LIMIT 5;
