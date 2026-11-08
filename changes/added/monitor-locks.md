- The Monitor tab's Locks view (`3`) shows who waits for whom as a tree, root blockers first:
  what each session waits for and how long, what its blocker holds, prepared transactions
  that hold locks, and on MySQL the holders of metadata locks a request cannot be granted
  beside. `b` goes to the root blocker of the selected wait.
