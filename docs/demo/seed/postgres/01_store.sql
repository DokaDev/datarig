-- The demo database of the README recordings (docs/demo): a small, made-up online store.
-- Every name, email and address is invented (emails use example.com). The rows are generated
-- from fixed arithmetic, never random(), so every render shows the same data.

CREATE SCHEMA store;

CREATE TABLE store.customers (
    id          integer     PRIMARY KEY,
    name        text        NOT NULL,
    email       text        NOT NULL UNIQUE,
    city        text        NOT NULL,
    tier        text        NOT NULL CHECK (tier IN ('basic', 'plus', 'gold')),
    newsletter  boolean     NOT NULL DEFAULT false,
    created_at  timestamptz NOT NULL
);

CREATE TABLE store.products (
    id        integer        PRIMARY KEY,
    sku       text           NOT NULL UNIQUE,
    name      text           NOT NULL,
    category  text           NOT NULL,
    price     numeric(10, 2) NOT NULL CHECK (price >= 0),
    stock     integer        NOT NULL DEFAULT 0
);
CREATE INDEX products_category_idx ON store.products (category);

CREATE TABLE store.orders (
    id           integer        PRIMARY KEY,
    customer_id  integer        NOT NULL REFERENCES store.customers (id),
    status       text           NOT NULL CHECK (status IN ('pending', 'paid', 'shipped', 'delivered', 'cancelled')),
    total        numeric(12, 2) NOT NULL,
    currency     text           NOT NULL DEFAULT 'USD',
    channel      text           NOT NULL,
    carrier      text,
    tracking_no  text,
    coupon       text,
    placed_at    timestamptz    NOT NULL,
    shipped_at   timestamptz,
    delivered_at timestamptz,
    ship_city    text           NOT NULL,
    gift_note    text
);
CREATE INDEX orders_customer_idx ON store.orders (customer_id);
CREATE INDEX orders_placed_at_idx ON store.orders (placed_at);

CREATE TABLE store.order_items (
    order_id    integer        NOT NULL REFERENCES store.orders (id),
    line        smallint       NOT NULL,
    product_id  integer        NOT NULL REFERENCES store.products (id),
    quantity    integer        NOT NULL CHECK (quantity > 0),
    unit_price  numeric(10, 2) NOT NULL,
    PRIMARY KEY (order_id, line)
);
CREATE INDEX order_items_product_idx ON store.order_items (product_id);

CREATE VIEW store.revenue_by_category AS
SELECT p.category, count(DISTINCT o.id) AS orders, sum(i.quantity * i.unit_price) AS revenue
FROM store.order_items i
JOIN store.orders o ON o.id = i.order_id
JOIN store.products p ON p.id = i.product_id
WHERE o.status <> 'cancelled'
GROUP BY p.category;

-- ── data ─────────────────────────────────────────────────────────────
WITH first AS (SELECT ARRAY['Ada','Bram','Cora','Dex','Elsa','Finn','Gale','Hugo','Iris','Juno',
                            'Kip','Lena','Milo','Nora','Otto','Pia','Quin','Rhea','Saul','Tess'] a),
     last  AS (SELECT ARRAY['Quillfeather','Thistlewood','Brightwater','Larkspur','Mossgrove',
                            'Fernsby','Copperkettle','Driftwood','Ambervale','Honeycomb',
                            'Starling','Pebbleton','Wrenfield','Marblecrest','Juniper'] a),
     city  AS (SELECT ARRAY['Port Lumen','Eastbrook','Glenhaven','Riverton','Cedar Falls',
                            'Ashford Bay','Northmere','Silverlake'] a)
INSERT INTO store.customers (id, name, email, city, tier, newsletter, created_at)
SELECT g,
       first.a[1 + g % 20] || ' ' || last.a[1 + (g * 7) % 15],
       lower(first.a[1 + g % 20]) || '.' || lower(last.a[1 + (g * 7) % 15]) || g || '@example.com',
       city.a[1 + (g * 7) % 8],
       (ARRAY['basic','basic','basic','plus','plus','gold'])[1 + (g * 13) % 6],
       g % 3 = 0,
       timestamptz '2025-01-01 09:00:00+00' + (g * 4073) * interval '1 second'
FROM generate_series(1, 2000) g, first, last, city;

WITH cat AS (SELECT ARRAY['Tents','Backpacks','Lanterns','Cookware','Footwear','Maps'] a),
     adj AS (SELECT ARRAY['Trail','Summit','Canyon','Harbor','Meadow','Glacier','Ember','Pine'] a),
     kind AS (SELECT ARRAY['Tent','Pack','Lantern','Pot','Boot','Map'] a)
INSERT INTO store.products (id, sku, name, category, price, stock)
SELECT g,
       'SKU-' || lpad(g::text, 4, '0'),
       adj.a[1 + (g / 6) % 8] || ' ' || kind.a[1 + g % 6] || ' ' || (1 + g / 48),
       cat.a[1 + g % 6],
       round((9.5 + (g * 37) % 240 + ((g * 11) % 100) / 100.0)::numeric, 2),
       (g * 53) % 400
FROM generate_series(1, 120) g, cat, adj, kind;

WITH city AS (SELECT ARRAY['Port Lumen','Eastbrook','Glenhaven','Riverton','Cedar Falls',
                           'Ashford Bay','Northmere','Silverlake'] a)
INSERT INTO store.orders (id, customer_id, status, total, channel, carrier, tracking_no, coupon,
                          placed_at, shipped_at, delivered_at, ship_city, gift_note)
SELECT g,
       1 + (g * 7919) % 2000,
       s.status,
       0,
       (ARRAY['web','web','app','app','phone'])[1 + (g * 3) % 5],
       CASE WHEN s.status IN ('shipped','delivered') THEN (ARRAY['Swift Parcel','Heron Express','Blue Route'])[1 + g % 3] END,
       CASE WHEN s.status IN ('shipped','delivered') THEN 'TRK' || lpad(((g * 104729) % 1000000)::text, 6, '0') END,
       CASE WHEN g % 9 = 0 THEN (ARRAY['WELCOME10','SPRING15','TRAIL5'])[1 + g % 3] END,
       timestamptz '2026-01-01 08:00:00+00' + (g * 1297) * interval '1 second',
       CASE WHEN s.status IN ('shipped','delivered')
            THEN timestamptz '2026-01-01 08:00:00+00' + (g * 1297 + 86400 + (g % 5) * 3600) * interval '1 second' END,
       CASE WHEN s.status = 'delivered'
            THEN timestamptz '2026-01-01 08:00:00+00' + (g * 1297 + 3 * 86400 + (g % 7) * 3600) * interval '1 second' END,
       city.a[1 + (g * 5) % 8],
       CASE WHEN g % 23 = 0 THEN 'Happy trails!' END
FROM generate_series(1, 20000) g
CROSS JOIN city
CROSS JOIN LATERAL (SELECT CASE
    WHEN g > 19900 THEN 'pending'
    WHEN g > 19600 THEN 'paid'
    WHEN g % 37 = 0 THEN 'cancelled'
    WHEN g > 18000 THEN 'shipped'
    ELSE 'delivered' END AS status) s;

INSERT INTO store.order_items (order_id, line, product_id, quantity, unit_price)
SELECT o.id, l, p.id, 1 + (o.id * l) % 3, p.price
FROM store.orders o
CROSS JOIN generate_series(1, 3) l
JOIN store.products p ON p.id = 1 + (o.id * 31 + l * 17) % 120
WHERE l <= 1 + o.id % 3;

UPDATE store.orders o
SET total = t.total
FROM (SELECT order_id, sum(quantity * unit_price) AS total FROM store.order_items GROUP BY order_id) t
WHERE t.order_id = o.id;

-- Back in id order after the update, so a table opened without ORDER BY lists 1, 2, 3, ...
CLUSTER store.orders USING orders_pkey;

ANALYZE;
