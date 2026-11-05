-- The MySQL demo database of the README recordings (docs/demo): made-up billing data. Every
-- name and email is invented (emails use example.org). Rows come from fixed arithmetic, so
-- every render shows the same data.

USE billing;

SET SESSION cte_max_recursion_depth = 100000;

CREATE TABLE accounts (
    id          INT          PRIMARY KEY,
    company     VARCHAR(80)  NOT NULL,
    contact     VARCHAR(80)  NOT NULL,
    email       VARCHAR(120) NOT NULL UNIQUE,
    plan        ENUM('starter', 'team', 'business') NOT NULL,
    seats       INT          NOT NULL,
    created_at  DATETIME     NOT NULL
);

CREATE TABLE invoices (
    id          INT            PRIMARY KEY,
    account_id  INT            NOT NULL,
    number      VARCHAR(20)    NOT NULL UNIQUE,
    status      ENUM('draft', 'open', 'paid', 'void') NOT NULL,
    amount      DECIMAL(12, 2) NOT NULL,
    issued_on   DATE           NOT NULL,
    due_on      DATE           NOT NULL,
    KEY invoices_account_idx (account_id),
    KEY invoices_status_idx (status),
    CONSTRAINT invoices_account_fk FOREIGN KEY (account_id) REFERENCES accounts (id)
);

CREATE TABLE payments (
    id          INT            PRIMARY KEY,
    invoice_id  INT            NOT NULL,
    method      ENUM('card', 'transfer', 'voucher') NOT NULL,
    amount      DECIMAL(12, 2) NOT NULL,
    paid_at     DATETIME       NOT NULL,
    KEY payments_invoice_idx (invoice_id),
    CONSTRAINT payments_invoice_fk FOREIGN KEY (invoice_id) REFERENCES invoices (id)
);

INSERT INTO accounts (id, company, contact, email, plan, seats, created_at)
WITH RECURSIVE n (g) AS (SELECT 1 UNION ALL SELECT g + 1 FROM n WHERE g < 400)
SELECT g,
       CONCAT(ELT(1 + g % 10, 'Lumen', 'Brightwater', 'Cobalt', 'Juniper', 'Harbor',
                  'Meadowlark', 'Ironleaf', 'Saffron', 'Northwind', 'Quartz'), ' ',
              ELT(1 + (g DIV 10) % 8, 'Labs', 'Works', 'Studio', 'Supply', 'Logistics',
                  'Foods', 'Analytics', 'Robotics')),
       CONCAT(ELT(1 + g % 12, 'Ada', 'Bram', 'Cora', 'Dex', 'Elsa', 'Finn', 'Gale', 'Hugo',
                  'Iris', 'Juno', 'Kip', 'Lena'), ' ',
              ELT(1 + (g * 5) % 9, 'Quillfeather', 'Thistlewood', 'Larkspur', 'Mossgrove',
                  'Fernsby', 'Driftwood', 'Ambervale', 'Starling', 'Wrenfield')),
       CONCAT('billing', g, '@example.org'),
       ELT(1 + (g * 7) % 3, 'starter', 'team', 'business'),
       1 + (g * 13) % 50,
       TIMESTAMP('2025-01-01 09:00:00') + INTERVAL g * 21601 SECOND
FROM n;

INSERT INTO invoices (id, account_id, number, status, amount, issued_on, due_on)
WITH RECURSIVE n (g) AS (SELECT 1 UNION ALL SELECT g + 1 FROM n WHERE g < 12000)
SELECT g,
       1 + (g * 7919) % 400,
       CONCAT('INV-', LPAD(g, 6, '0')),
       CASE WHEN g > 11800 THEN 'draft' WHEN g > 11000 THEN 'open'
            WHEN g % 41 = 0 THEN 'void' ELSE 'paid' END,
       ROUND(49 + (g * 37) % 1900 + ((g * 11) % 100) / 100, 2),
       DATE('2025-06-01') + INTERVAL g DIV 24 DAY,
       DATE('2025-06-01') + INTERVAL g DIV 24 + 30 DAY
FROM n;

INSERT INTO payments (id, invoice_id, method, amount, paid_at)
SELECT id, id, ELT(1 + id % 3, 'card', 'transfer', 'voucher'), amount,
       TIMESTAMP(issued_on) + INTERVAL 9 + id % 9 HOUR + INTERVAL id % 20 DAY
FROM invoices
WHERE status = 'paid';

ANALYZE TABLE accounts, invoices, payments;
