- A read-only PostgreSQL environment can allow functions that are not built-ins with
  `read_only_functions` (such as `["st_x", "public.crypt"]`) and the functions of extensions
  with `read_only_extensions` (such as `["postgis"]`). A refused call says how to allow it, and
  `:session` lists what is allowed.
  The extensions' functions are read from the profile's own database when it connects; a
  refresh of the profile in the explorer reads them again after a failure.
