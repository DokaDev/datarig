- MySQL: when a cancel sent again for a statement got no answer, a result whose rows were held
  for paging was cut after its first page and the run ended as a lost connection. Its rows and
  its end are now read as you page, and the session closes after that.
