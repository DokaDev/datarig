use super::Checks;

fn first() -> Checks {
    let mut c = Checks::default();
    c.scenario("editor");
    c.check("editor typing_ms p95", 30.0, 25.0, " ms");
    c.check("editor RSS", 100.0, 160.0, " MiB");
    c.scenario("export_stream");
    c.check_min("export_stream rows per second", 40_000.0, 50_000.0, "");
    c.scenario("startup");
    c.check("startup first frame p95", 4.0, 250.0, " ms");
    c
}

#[test]
fn only_the_scenarios_with_a_time_over_budget_run_again() {
    assert_eq!(first().timing_failed(), ["editor", "export_stream"]);
    let mut counts = Checks::default();
    counts.scenario("rtt");
    counts.check("rtt insert total", 2.0, 1.0, " round trips");
    counts.scenario("paging");
    counts.missing("paging RSS peak");
    assert!(counts.timing_failed().is_empty());
}

#[test]
fn a_time_over_budget_twice_fails() {
    let mut again = Checks::default();
    again.scenario("editor");
    again.check("editor typing_ms p95", 31.0, 25.0, " ms");
    again.scenario("export_stream");
    again.check_min("export_stream rows per second", 45_000.0, 50_000.0, "");
    assert_eq!(first().failed(&again), ["editor typing_ms p95", "export_stream rows per second"]);
}

#[test]
fn a_time_within_budget_when_measured_again_passes() {
    let mut again = Checks::default();
    again.scenario("editor");
    again.check("editor typing_ms p95", 2.0, 25.0, " ms");
    again.scenario("export_stream");
    again.check_min("export_stream rows per second", 400_000.0, 50_000.0, "");
    assert!(first().failed(&again).is_empty());
}

#[test]
fn a_time_not_measured_again_fails() {
    let mut again = Checks::default();
    again.scenario("editor");
    again.missing("editor typing_ms p95");
    assert_eq!(
        first().failed(&again),
        ["editor typing_ms p95", "export_stream rows per second"],
        "a check missing on the second run is no pass, and the scenario never run again fails too"
    );
}

#[test]
fn counts_are_never_measured_again() {
    let mut c = Checks::default();
    c.scenario("editor");
    c.check("editor typing_ms p95", 30.0, 25.0, " ms");
    c.check("editor RSS", 200.0, 160.0, " MiB");
    c.check("editor / miss passes", 2.0, 1.0, " passes");
    c.missing("editor lines highlighted per frame");
    let mut again = Checks::default();
    again.scenario("editor");
    again.check("editor typing_ms p95", 2.0, 25.0, " ms");
    again.check("editor RSS", 100.0, 160.0, " MiB");
    again.check("editor / miss passes", 1.0, 1.0, " passes");
    again.check("editor lines highlighted per frame", 40.0, 45.0, " lines");
    assert_eq!(
        c.failed(&again),
        ["editor RSS", "editor / miss passes", "editor lines highlighted per frame"],
        "a count over budget the first time fails even when it holds the second time"
    );
}

#[test]
fn a_count_over_budget_on_the_second_run_fails() {
    let mut again = Checks::default();
    again.scenario("editor");
    again.check("editor typing_ms p95", 2.0, 25.0, " ms");
    again.check("editor RSS", 200.0, 160.0, " MiB");
    again.scenario("export_stream");
    again.check_min("export_stream rows per second", 400_000.0, 50_000.0, "");
    assert_eq!(first().failed(&again), ["editor RSS"]);
}

#[test]
fn without_a_second_run_every_check_over_budget_fails() {
    assert_eq!(first().failed(&Checks::default()), ["editor typing_ms p95", "export_stream rows per second"]);
}
