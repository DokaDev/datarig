- MySQL and MariaDB: a row count or an export inside your own transaction at SERIALIZABLE (or at
  a level the app cannot tell) is refused instead of reading every row with a shared lock that
  other sessions' writes wait for until the transaction ends.
