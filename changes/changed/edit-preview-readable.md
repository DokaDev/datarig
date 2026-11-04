- The preview of pending changes shows each statement in the editor's syntax colors: a long one
  wraps over as many lines as it needs, its expected count on its first line, and a literal of
  more than a few hundred bytes shows its start and end around how many bytes are left out. On
  MySQL, a statement that compares text by its bytes (`_utf8mb4 X'…'`) is followed by a dim
  `↳ set column = 'text' · where column = 'text'` line that reads the values back; that line is
  not SQL, and Copy SQL copies the statements whole without it.
