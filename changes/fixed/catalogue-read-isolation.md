- On a PostgreSQL server whose role or database defaults to SERIALIZABLE, the explorer's and
  a table tab's reads of the catalogue no longer run as serializable transactions: they
  waited for a safe snapshot under `default_transaction_deferrable` and failed on a standby.
