- MySQL: a cancel that lands while the server looks up the definer of a `SQL SECURITY DEFINER`
  routine now ends the run as cancelled, instead of with error 1449 (the definer does not exist).
