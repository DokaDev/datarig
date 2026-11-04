- The preview of pending changes shows each statement whole, in the editor's syntax colors:
  a long one wraps over as many lines as it needs, its expected count on its first line. On
  MySQL, a statement that compares text by its bytes (`_utf8mb4 X'…'`) is followed by a dim
  `↳ column = 'text'` line that reads the values back; that line is not SQL and Copy SQL leaves
  it out.
