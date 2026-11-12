- A Startup SQL that sets the server's idle limit (`idle_session_timeout`, `wait_timeout`) no
  longer races the server: datarig ends those sessions a little earlier than the server would
  (a tenth of the limit, from 1 to 30 seconds) and says why.
