- The MySQL monitor reads row lock waits only when its connection confirms the server's version
  and reaches the server directly. Behind a proxy that reports another version, or a pooler, the
  Locks view says they are not read instead of risking a stalled server.
