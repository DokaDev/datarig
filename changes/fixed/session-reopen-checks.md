- A lost metadata session, the metadata sessions of other databases and a Monitor tab's session
  now open only after the same checks as a connect (the profile's environment, its tunnel
  setting, its Startup SQL), so a profile changed since it connected no longer opens new
  sessions behind them.
