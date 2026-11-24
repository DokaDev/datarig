- On MySQL, editing rows that share the same values no longer offers to change one or all of
  them when the structure was read by a session that reports a different server or version than
  the one that would apply the edit; such an edit is refused with the reason.
