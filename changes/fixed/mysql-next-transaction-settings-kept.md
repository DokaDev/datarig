- MySQL: after `SET TRANSACTION` without `SESSION`, which sets your next transaction only, an
  export or a row count no longer uses that setting up. Until a transaction starts or ends they
  are refused with a note saying how to run them.
