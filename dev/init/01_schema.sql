-- datarig development and test database: schemas, tables, constraints, views
-- Compatible with PostgreSQL 14+ (docker image uses 17)

CREATE SCHEMA IF NOT EXISTS shop;
CREATE SCHEMA IF NOT EXISTS analytics;

-- ── shop ────────────────────────────────────────────────────────────
CREATE TABLE shop.users (
    id           bigserial PRIMARY KEY,
    email        text        NOT NULL UNIQUE,
    name         text        NOT NULL,
    nickname     text,                         -- nullable, often has emoji
    address      text,
    phone        text,
    bio          text,                         -- very long text for some rows
    profile      jsonb       NOT NULL DEFAULT '{}'::jsonb,
    is_active    boolean     NOT NULL DEFAULT true,
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX users_created_at_idx ON shop.users (created_at);

CREATE TABLE shop.products (
    id           bigserial PRIMARY KEY,
    sku          text           NOT NULL UNIQUE,
    name         text           NOT NULL,
    category     text           NOT NULL,
    price        numeric(12, 2) NOT NULL CHECK (price >= 0),
    stock        integer        NOT NULL DEFAULT 0,
    description  text,
    attributes   jsonb,
    created_at   timestamptz    NOT NULL DEFAULT now()
);
CREATE INDEX products_category_idx ON shop.products (category);

CREATE TABLE shop.orders (
    id            bigserial PRIMARY KEY,
    user_id       bigint         NOT NULL REFERENCES shop.users (id),
    status        text           NOT NULL CHECK (status IN ('pending', 'paid', 'shipped', 'cancelled')),
    total_amount  numeric(14, 2) NOT NULL DEFAULT 0,
    memo          text,
    shipping      jsonb,
    ordered_at    timestamptz    NOT NULL DEFAULT now()
);
CREATE INDEX orders_user_id_idx ON shop.orders (user_id);
CREATE INDEX orders_status_ordered_at_idx ON shop.orders (status, ordered_at);

CREATE TABLE shop.order_items (
    order_id    bigint         NOT NULL REFERENCES shop.orders (id) ON DELETE CASCADE,
    line_no     integer        NOT NULL,
    product_id  bigint         NOT NULL REFERENCES shop.products (id),
    quantity    integer        NOT NULL CHECK (quantity > 0),
    unit_price  numeric(12, 2) NOT NULL,
    PRIMARY KEY (order_id, line_no)                -- composite PK
);
CREATE INDEX order_items_product_id_idx ON shop.order_items (product_id);

CREATE TABLE shop.reviews (
    id          bigserial PRIMARY KEY,
    product_id  bigint      NOT NULL REFERENCES shop.products (id),
    user_id     bigint      NOT NULL REFERENCES shop.users (id),
    rating      smallint    NOT NULL CHECK (rating BETWEEN 1 AND 5),
    title       text,
    body        text,
    created_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (product_id, user_id)
);

-- Table WITHOUT a primary key (edit-safety tests later; read-only in grid)
CREATE TABLE shop.audit_log (
    occurred_at  timestamptz NOT NULL DEFAULT now(),
    actor        text,
    action       text        NOT NULL,
    detail       jsonb
);

CREATE VIEW shop.order_summary AS
SELECT o.id          AS order_id,
       u.name        AS user_name,
       o.status,
       o.total_amount,
       count(oi.*)   AS item_count,
       o.ordered_at
FROM shop.orders o
JOIN shop.users u ON u.id = o.user_id
LEFT JOIN shop.order_items oi ON oi.order_id = o.id
GROUP BY o.id, u.name;

-- ── analytics ───────────────────────────────────────────────────────
-- Big table for streaming / row-limit / cancel tests (populated in 03_big.sql)
CREATE TABLE analytics.events (
    id          bigint      PRIMARY KEY,
    user_id     bigint,
    event_type  text        NOT NULL,
    page        text,
    payload     jsonb,
    created_at  timestamptz NOT NULL
);

CREATE TABLE analytics.daily_stats (
    stat_date   date    NOT NULL,
    metric      text    NOT NULL,
    value       numeric NOT NULL,
    PRIMARY KEY (stat_date, metric)
);

-- Helper for cancel tests:  SELECT analytics.slow(30);
CREATE FUNCTION analytics.slow(seconds int DEFAULT 30)
RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_sleep(seconds);
    RETURN 'done after ' || seconds || 's';
END;
$$;
