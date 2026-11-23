- On MariaDB, a statement that takes a sequence's next value (`NEXT VALUE FOR`, `NEXTVAL()`,
  `SETVAL()`, `s.nextval`) is treated as a change: a read-only profile refuses it before it is
  sent, and a result's next page never runs it again.
