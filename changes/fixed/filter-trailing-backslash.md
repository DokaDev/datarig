- On a MySQL or MariaDB table tab, a filter that ends in a backslash outside a string (for
  example `:where id = 1 \`) no longer closes the app. It is refused with the backslash
  marked, as is a quoted variable name left open (`@'x`).
