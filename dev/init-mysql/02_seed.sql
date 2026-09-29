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

-- ── Korean rows ─────────────────────────────────────────────────────
-- Korean text is written as _utf8mb4 X'' hex literals (the UTF-8 bytes) that the server
-- decodes when it loads the file, so no Hangul appears in the repository; the English comment
-- above each row says what it reads.

INSERT INTO users (email, name, nickname, address, phone, bio, profile, is_active, created_at) VALUES
-- Korean: Hong Gil-dong; nickname "Gil-dong"; Sejong-daero, Jongno-gu, Seoul; bio "Hello. I am a developer who likes databases."
('hong@example.com', _utf8mb4 X'ED998DEAB8B8EB8F99', _utf8mb4 X'EAB8B8EB8F99EC9DB4', _utf8mb4 X'EC849CEC9AB8ED8AB9EBB384EC8B9C20ECA285EBA19CEAB5AC20EC84B8ECA285EB8C80EBA19C20313735', '010-2024-1009', _utf8mb4 X'EC9588EB8595ED9598EC84B8EC9A942E20EB8DB0EC9DB4ED84B0EBB2A0EC9DB4EC8AA4EBA5BC20ECA28BEC9584ED9598EB8A9420EAB09CEBB09CEC9E90EC9E85EB8B88EB8BA42E', JSON_OBJECT('lang', 'ko', 'city', _utf8mb4 X'EC849CEC9AB8'), 1, '2024-09-10 09:00:00'),
-- Korean: Kim Min-ji; nickname mixes Latin, emoji and Hangul; Haeundae, Busan; very long Korean bio
('minji@example.com', _utf8mb4 X'EAB980EBAFBCECA780', _utf8mb4 X'6D696E6A6920E29CA820EBAFBCECA780', _utf8mb4 X'EBB680EC82B0EAB491EC97ADEC8B9C20ED95B4EC9AB4EB8C80EAB5AC20ED95B4EC9AB4EB8C80ED95B4EBB380EBA19C20323634', NULL, REPEAT(_utf8mb4 X'EC9584ECA3BC20EAB8B420EC9E90EAB8B0EC868CEAB09CEC9E85EB8B88EB8BA42E20ED959CEAB5ADEC96B420EBACB8EC9EA5EC9DB420EAB384EC868D20EC9DB4EC96B4ECA791EB8B88EB8BA42E20', 120), JSON_OBJECT('lang', 'ko', 'city', _utf8mb4 X'EBB680EC82B0'), 1, '2024-10-11 10:11:12'),
-- Korean: Lee Seo-jun; Jeju address with emoji; bio mixes Korean, English, Japanese, Chinese and a flag
('seojun@example.com', _utf8mb4 X'EC9DB4EC849CECA480', NULL, _utf8mb4 X'ECA09CECA3BCED8AB9EBB384EC9E90ECB998EB8F8420ECA09CECA3BCEC8B9C20ECB2A8EB8BA8EBA19C2032343220F09F8F9DEFB88F', '+82-10-5555-0242', _utf8mb4 X'ED959CEAB5ADEC96B420456E676C69736820E697A5E69CACE8AA9E20E4B8ADE6968720F09F87B0F09F87B720EC849EEC9DB820ED858DEC8AA4ED8AB820E29C85', JSON_OBJECT('lang', 'ko', 'city', _utf8mb4 X'ECA09CECA3BC'), 0, '2024-11-12 13:14:15'),
-- Korean: name is "Hangul" in decomposed jamo (NFD, 6 code points), nickname uses compatibility jamo (laughter), bio says "The name of this row is in NFD (decomposed jamo) form."
('nfd@example.com', _utf8mb4 X'E18492E185A1E186ABE18480E185B3E186AF204E4644', _utf8mb4 X'E3858BE3858BE3858B20F09F9882', NULL, NULL, _utf8mb4 X'EC9DB420ED9689EC9D9820EC9DB4EBA684EC9D80204E464428EC9E90EBAAA820EBB684ED95B42920ED9895ED839CEC9E85EB8B88EB8BA42E', JSON_OBJECT('lang', 'ko'), 1, '2024-12-13 16:17:18');

INSERT INTO products (sku, name, category, price, stock, description, attributes, created_at) VALUES
-- Korean: Wireless Earbuds Pro, home appliances; "Noise cancelling. Bluetooth 5.3, up to 30 hours of playback"
('SKU-KR-0001', _utf8mb4 X'EBACB4EC84A020EC9DB4EC96B4ED8FB020ED9484EBA19C', _utf8mb4 X'EAB080ECA084', 189000.00, 120, _utf8mb4 X'EB85B8EC9DB4ECA68820ECBA94EC8AACEBA78120ECA780EC9B902E20426C7565746F6F746820352E332C20ECB59CEB8C80203330EC8B9CEAB08420EC9EACEC839D20F09F8EA7', JSON_OBJECT('color', 'black', 'origin', 'KR'), '2024-09-01 00:00:00'),
-- Korean: Jeju tangerines 5kg, food; out of stock, no description
('SKU-KR-0002', _utf8mb4 X'ECA09CECA3BC20EAB090EAB7A420356B6720F09F8D8A', _utf8mb4 X'EC8B9DED9288', 32900.00, 0, NULL, NULL, '2024-09-02 00:00:00'),
-- Korean: Solid wood desk, furniture; very long description; color "walnut"
('SKU-KR-0003', _utf8mb4 X'EC9B90EBAAA920ECB185EC8381202831323030C39736303029', _utf8mb4 X'EAB080EAB5AC', 459000.00, 7, REPEAT(_utf8mb4 X'ED8ABCED8ABCED959C20EC9B90EBAAA9EC9CBCEBA19C20EBA78CEB93A020ECB185EC8381EC9E85EB8B88EB8BA42E20', 60), JSON_OBJECT('color', _utf8mb4 X'EC9B94EB849B', 'origin', 'KR'), '2024-09-03 00:00:00'),
-- Korean: Limited edition board game "Hangul Play", toys; "2-6 players, ages 8+. 128 jamo cards included"
('SKU-KR-0004', _utf8mb4 X'ED959CECA095ED8C9020EBB3B4EB939CEAB28CEC9E8420E3808CED959CEAB88020EB8680EC9DB4E3808D', _utf8mb4 X'EC9984EAB5AC', 45000.00, 33, _utf8mb4 X'327E36EC9DB8EC9AA92C2038EC84B820EC9DB4EC83812E20ED959CEAB88020EC9E90EBAAA820ECB9B4EB939C20313238EC9EA520ED8FACED95A8', JSON_OBJECT('color', 'white', 'origin', 'KR'), '2024-09-04 00:00:00');

INSERT INTO reviews (product_id, user_id, rating, title, body, created_at)
-- Korean: title "The best", body "Great sound and long battery life. Highly recommended!"
SELECT p.id, u.id, 5, _utf8mb4 X'ECB59CEAB3A0EC9888EC9A9420F09F918D', _utf8mb4 X'EC9D8CECA788EC9DB420ECA095EBA79020ECA28BEAB3A020EBB0B0ED84B0EBA6ACEB8F8420EC98A4EB9E98EAB080EC9A942E20EAB095EBA0A520ECB694ECB29CED95A9EB8B88EB8BA421', TIMESTAMP '2024-09-20 20:00:00'
FROM products p, users u WHERE p.sku = 'SKU-KR-0001' AND u.email = 'hong@example.com'
UNION ALL
-- Korean: title "So-so", very long body "It was a little uncomfortable to wear."
SELECT p.id, u.id, 2, _utf8mb4 X'EAB7B8ECA08020EAB7B8EB9E98EC9A94', REPEAT(_utf8mb4 X'ECB0A9EC9AA9EAB090EC9DB420ECA1B0EAB88820EBB688ED8EB8ED9688EC96B4EC9A942E20', 80), TIMESTAMP '2024-10-20 21:00:00'
FROM products p, users u WHERE p.sku = 'SKU-KR-0001' AND u.email = 'minji@example.com'
UNION ALL
-- Korean: no title, body mixes English and Korean: "Delivery was fast but assembly is hard"
SELECT p.id, u.id, 4, NULL, _utf8mb4 X'44656C697665727920EBB9A8EB9E90EC96B4EC9A9420F09F9A9A2062757420ECA1B0EBA6BDEC9DB420EC96B4EBA0A4EC9B80', TIMESTAMP '2024-11-20 22:00:00'
FROM products p, users u WHERE p.sku = 'SKU-KR-0003' AND u.email = 'seojun@example.com'
UNION ALL
-- Korean: title "Fast delivery", no body
SELECT p.id, u.id, 3, _utf8mb4 X'EBB0B0EC86A1EC9DB420EBB9A8EB9DBCEC9A9420F09F9A9A', NULL, TIMESTAMP '2024-12-20 23:00:00'
FROM products p, users u WHERE p.sku = 'SKU-KR-0004' AND u.email = 'nfd@example.com';

-- Korean: memo "Please leave it at the door", shipping method "parcel delivery"
INSERT INTO orders (user_id, status, total_amount, memo, shipping, ordered_at)
SELECT id, 'paid', 0, _utf8mb4 X'EBACB820EC959EEC979020EB8693EC958420ECA3BCEC84B8EC9A9420F09F998F', JSON_OBJECT('method', _utf8mb4 X'ED839DEBB0B0', 'fee', 3000), '2024-09-15 12:00:00' FROM users WHERE email = 'hong@example.com';
INSERT INTO order_items (order_id, line_no, product_id, quantity, unit_price)
SELECT o.id, l.line_no, p.id, l.qty, p.price
FROM orders o JOIN users u ON u.id = o.user_id AND u.email = 'hong@example.com',
     (SELECT 1 AS line_no, 'SKU-KR-0001' AS sku, 1 AS qty UNION ALL SELECT 2, 'SKU-KR-0004', 2) l
JOIN products p ON p.sku = l.sku;
UPDATE orders o
JOIN (SELECT order_id, SUM(quantity * unit_price) AS total FROM order_items GROUP BY order_id) s ON s.order_id = o.id
JOIN users u ON u.id = o.user_id AND u.email = 'hong@example.com'
SET o.total_amount = s.total;

-- Korean: note "administrator work"
INSERT INTO audit_log (occurred_at, actor, action, detail)
VALUES ('2024-09-30 18:00:00', 'admin_kr', 'export', JSON_OBJECT('ip', '10.0.82.1', 'note', _utf8mb4 X'EAB480EBA6ACEC9E9020EC9E91EC9785'));
