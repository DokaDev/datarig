# datarig

datarig is a terminal database client for PostgreSQL and MySQL: several connections side by
side, a schema explorer, a SQL editor with vim keys and a results grid that pages through large
results instead of loading them whole.

It is early, pre-1.0 software: the configuration format, key bindings and behavior may still
change between releases. The [README](https://github.com/DokaDev/datarig#readme) is the full
guide for now (install, features, configuration, safety); this site is being built up from it.

The reference pages are generated from the code and checked by the test suite, so they match
the version they were built from:

- [Keybindings](reference/keybindings.md): every key, by context.
- [Commands](reference/commands.md): the `:` command line and `:set`.
- [Theme file format](reference/theme-files.md): writing your own theme.
