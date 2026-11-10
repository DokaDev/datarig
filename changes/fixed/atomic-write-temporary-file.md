- datarig's own files (the workspace, settings and other state) are now written through a
  temporary file created exclusively under an unpredictable name, so a symbolic link placed
  next to them can no longer redirect a write. A file name of the longest length a file
  system allows can now be saved too.
