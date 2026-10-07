-- datarig development and test database (MySQL): schemas, tables, constraints, views
-- Compatible with MySQL 8.0+ (docker image uses 8.4 LTS)
-- utf8mb4 / utf8mb4_0900_ai_ci everywhere so CJK text and emoji round-trip correctly.

-- The image loads these files with a client in the POSIX locale (latin1): say what they are.
SET NAMES utf8mb4;

CREATE DATABASE IF NOT EXISTS shop
    CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci;
GRANT ALL PRIVILEGES ON shop.* TO 'datarig'@'%';
FLUSH PRIVILEGES;

USE shop;

CREATE TABLE users (
    id           BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
    email        VARCHAR(255) NOT NULL UNIQUE,
    name         VARCHAR(255) NOT NULL,
    nickname     VARCHAR(255),                 -- nullable, often has emoji
    address      TEXT,
    phone        VARCHAR(50),
    bio          TEXT,                         -- very long text for some rows
    profile      JSON NOT NULL,
    is_active    TINYINT(1) NOT NULL DEFAULT 1,
    created_at   DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    INDEX users_created_at_idx (created_at)
) ENGINE=InnoDB CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci;

CREATE TABLE products (
    id           BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
    sku          VARCHAR(64) NOT NULL UNIQUE,
    name         VARCHAR(255) NOT NULL,
    category     VARCHAR(100) NOT NULL,
    price        DECIMAL(12, 2) NOT NULL CHECK (price >= 0),
    stock        INT NOT NULL DEFAULT 0,
    description  TEXT,
    attributes   JSON,
    created_at   DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    INDEX products_category_idx (category)
) ENGINE=InnoDB CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci;

CREATE TABLE orders (
    id            BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
    user_id       BIGINT UNSIGNED NOT NULL,
    status        ENUM('pending', 'paid', 'shipped', 'cancelled') NOT NULL,
    total_amount  DECIMAL(14, 2) NOT NULL DEFAULT 0,
    memo          TEXT,
    shipping      JSON,
    ordered_at    DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT orders_user_id_fk FOREIGN KEY (user_id) REFERENCES users (id),
    INDEX orders_user_id_idx (user_id),
    INDEX orders_status_ordered_at_idx (status, ordered_at)
) ENGINE=InnoDB CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci;

CREATE TABLE order_items (
    order_id    BIGINT UNSIGNED NOT NULL,
    line_no     INT NOT NULL,
    product_id  BIGINT UNSIGNED NOT NULL,
    quantity    INT NOT NULL CHECK (quantity > 0),
    unit_price  DECIMAL(12, 2) NOT NULL,
    PRIMARY KEY (order_id, line_no),               -- composite PK
    CONSTRAINT order_items_order_id_fk FOREIGN KEY (order_id) REFERENCES orders (id) ON DELETE CASCADE,
    CONSTRAINT order_items_product_id_fk FOREIGN KEY (product_id) REFERENCES products (id),
    INDEX order_items_product_id_idx (product_id)
) ENGINE=InnoDB CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci;

CREATE TABLE reviews (
    id          BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
    product_id  BIGINT UNSIGNED NOT NULL,
    user_id     BIGINT UNSIGNED NOT NULL,
    rating      TINYINT NOT NULL CHECK (rating BETWEEN 1 AND 5),
    title       VARCHAR(255),
    body        TEXT,
    created_at  DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE KEY reviews_product_user_uk (product_id, user_id),
    CONSTRAINT reviews_product_id_fk FOREIGN KEY (product_id) REFERENCES products (id),
    CONSTRAINT reviews_user_id_fk FOREIGN KEY (user_id) REFERENCES users (id)
) ENGINE=InnoDB CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci;

-- Table WITHOUT a primary key (edit-safety tests later; read-only in grid)
CREATE TABLE audit_log (
    occurred_at  DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    actor        VARCHAR(100),
    action       VARCHAR(100) NOT NULL,
    detail       JSON
) ENGINE=InnoDB CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci;

CREATE VIEW order_summary AS
SELECT o.id          AS order_id,
       u.name        AS user_name,
       o.status,
       o.total_amount,
       COUNT(oi.order_id) AS item_count,
       o.ordered_at
FROM orders o
JOIN users u ON u.id = o.user_id
LEFT JOIN order_items oi ON oi.order_id = o.id
GROUP BY o.id, u.name, o.status, o.total_amount, o.ordered_at;

-- Big table for streaming / row-limit tests (populated in 03_big.sql)
CREATE TABLE events (
    id          BIGINT UNSIGNED PRIMARY KEY,
    user_id     BIGINT UNSIGNED,
    event_type  VARCHAR(50) NOT NULL,
    page        VARCHAR(255),
    payload     JSON,
    created_at  DATETIME NOT NULL
) ENGINE=InnoDB CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci;
