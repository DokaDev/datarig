- Once a day datarig asks GitHub whether a newer release exists (one HTTPS request to
  api.github.com, with nothing about you or your databases; through `HTTPS_PROXY` when one is
  set). A newer one is told about at the next start, with the command that updates your
  install; `Esc` closes it (it comes back after a week), `s` skips that version, `o` turns the
  checks off. `:update` says what the last check found. Turn it off with `[updates] check =
  "off"` (settings screen, Updates), `:set updates.check=off` or `DATARIG_NO_UPDATE_CHECK=1`;
  there is no check on CI (`CI` set).
