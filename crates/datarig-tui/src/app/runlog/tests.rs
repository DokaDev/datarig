use super::*;

fn log(n: usize) -> RunLog {
    RunLog::new(&(1..=n).map(|i| format!("SELECT {i}")).collect::<Vec<_>>())
}

fn outcomes(l: &RunLog) -> Vec<StatementOutcome> {
    l.statements.iter().map(|s| s.outcome.clone()).collect()
}

#[test]
fn a_failure_in_the_middle_leaves_the_rest_not_run() {
    use StatementOutcome::*;
    let mut l = log(3);
    l.started(0);
    l.finished(0, &Outcome::Affected(2), Duration::from_millis(3));
    l.started(1);
    assert_eq!(l.running(), Some(1));
    assert_eq!(l.answered(Failed("ERROR: boom".into()), None), Some(1));
    assert_eq!(outcomes(&l), [Affected(2), Failed("ERROR: boom".into()), NotRun]);
    assert_eq!(l.failed(), Some((1, "ERROR: boom")));
    assert_eq!(l.statements[0].elapsed, Some(Duration::from_millis(3)));
}

#[test]
fn a_cancel_between_statements_belongs_to_the_next_one() {
    use StatementOutcome::*;
    let mut l = log(3);
    l.started(0);
    l.finished(0, &Outcome::Command("SELECT".into()), Duration::ZERO);
    // Cancelled before statement 2 started: nothing is running.
    assert_eq!(l.answered(Cancelled, None), Some(1));
    assert_eq!(outcomes(&l), [Command("SELECT".into()), Cancelled, NotRun]);
}

#[test]
fn one_statement_is_answered_without_progress_and_its_rows_grow() {
    use StatementOutcome::*;
    let mut l = log(1);
    assert!(!l.several());
    assert_eq!(l.answered(Rows { count: 500, more: true }, Some(Duration::from_millis(9))), Some(0));
    l.rows_fetched(1000, false);
    assert_eq!(outcomes(&l), [Rows { count: 1000, more: false }]);
    // An empty run has nothing to answer.
    assert_eq!(RunLog::default().answered(Cancelled, None), None);
}

#[test]
fn excerpts_name_a_statement_by_its_first_line() {
    assert_eq!(excerpt("  SELECT 1/0", 40), "SELECT 1/0");
    assert_eq!(excerpt("\n  UPDATE t\n  SET a = 1", 40), "UPDATE t…");
    assert_eq!(excerpt("SELECT '漢字のとても長い文字列'", 10), "SELECT '漢字…");
}
