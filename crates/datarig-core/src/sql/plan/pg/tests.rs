//! The fixtures are `EXPLAIN` output captured from PostgreSQL 13.23, 14.22, 15.19, 16.15, 17.11
//! and 18.6 (`fixtures/pg<major>/`): the tables of `fixtures/setup.sql`, each statement of
//! `fixtures/queries.txt` as `<name>.analyze.json` (`ANALYZE, BUFFERS, FORMAT JSON`),
//! `<name>.plan.json` (`FORMAT JSON`) and `<name>.plan.txt` (the text form of the same plan),
//! and `join.verbose.json` (`ANALYZE, BUFFERS, VERBOSE, SETTINGS`). They cover parallel workers,
//! subplans and InitPlans, a CTE, partitions, a trigger, a never executed node and JIT.

use super::*;
use crate::sql::plan::{Measure, text};

const VERSIONS: [u32; 6] = [13, 14, 15, 16, 17, 18];

fn fixture(version: u32, name: &str) -> String {
    let path = format!("{}/src/sql/plan/fixtures/pg{version}/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn names() -> Vec<String> {
    let q = fixture(13, "../queries.txt");
    q.lines().filter_map(|l| l.split_once('|').map(|(n, _)| n.to_string())).collect()
}

fn find(plan: &Plan, op: &str) -> usize {
    plan.nodes.iter().position(|n| n.op == op).unwrap_or_else(|| panic!("no {op}: {:?}", ops(plan)))
}

fn ops(plan: &Plan) -> Vec<String> {
    plan.nodes.iter().map(|n| n.op.clone()).collect()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn every_fixture_of_every_version_is_a_plan() {
    for v in VERSIONS {
        for name in names() {
            for (file, analyzed) in [(format!("{name}.analyze.json"), true), (format!("{name}.plan.json"), false)] {
                let plan = parse(&fixture(v, &file)).unwrap_or_else(|e| panic!("pg{v} {file}: {e:?}"));
                assert!(!plan.nodes.is_empty(), "pg{v} {file}");
                assert_eq!(plan.analyzed, analyzed, "pg{v} {file}");
                assert_eq!(plan.timed, analyzed, "pg{v} {file}");
                assert_eq!(plan.measure(), if analyzed { Measure::Time } else { Measure::Cost }, "pg{v} {file}");
                assert_eq!(plan.execution_ms.is_some(), analyzed, "pg{v} {file}");
                assert_eq!(plan.roots().count(), 1);
                for (i, n) in plan.nodes.iter().enumerate() {
                    assert!(!n.op.is_empty(), "pg{v} {file} node {i}");
                    assert!(n.cost.is_some() && n.plan_rows.is_some(), "pg{v} {file} node {i}");
                    if let Some(p) = n.parent {
                        assert!(p < i && plan.nodes[p].children.contains(&i), "depth first, parent first");
                        assert_eq!(n.depth, plan.nodes[p].depth + 1);
                    }
                    if analyzed {
                        assert!(n.self_ms.is_some_and(|s| s >= 0.0), "pg{v} {file} node {i}");
                        assert!(n.buffers.is_some(), "BUFFERS: pg{v} {file} node {i}");
                    }
                    assert!(n.self_cost.is_some_and(|s| s >= 0.0));
                }
                // Measured self times add up to the whole (a CTE's time is taken out of the
                // scans that read it, not counted twice). Costs need not: a `Limit` costs less
                // than what it stops early.
                if analyzed {
                    let sum: f64 = (0..plan.nodes.len()).map(|i| plan.share(i)).sum();
                    assert!(sum <= 1.0 + 1e-3, "pg{v} {file}: shares add up to {sum}");
                }
            }
        }
    }
}

/// The text written from a plan's JSON is the server's own text form of the same plan.
#[test]
fn the_text_from_the_json_is_the_servers_text() {
    for v in VERSIONS {
        for name in names() {
            let plan = parse(&fixture(v, &format!("{name}.plan.json"))).unwrap();
            let want = fixture(v, &format!("{name}.plan.txt"));
            assert_eq!(text::text(&plan), want.trim_end(), "pg{v} {name}");
        }
    }
}

#[test]
fn parallel_workers_count_once_not_per_process() {
    let plan = parse(&fixture(18, "parallel.analyze.json")).unwrap();
    assert_eq!(ops(&plan), ["Finalize HashAggregate", "Gather", "Partial HashAggregate", "Parallel Seq Scan"]);
    let [fin, gather, partial, scan] = [0, 1, 2, 3].map(|i| &plan.nodes[i]);
    assert_eq!(gather.workers, Some((2, Some(2))));
    assert_eq!(scan.target.as_deref(), Some("on zz_orders"));
    // Loops 3 = two workers and the leader, at the same time: the per-loop time once.
    assert!(close(partial.total_ms.unwrap(), 80.693), "{:?}", partial.total_ms);
    assert!(close(scan.total_ms.unwrap(), 26.031));
    assert!(close(partial.self_ms.unwrap(), 80.693 - 26.031));
    assert!(close(gather.self_ms.unwrap(), 121.280 - 80.693));
    assert!(close(fin.self_ms.unwrap(), 132.602 - 121.280));
    assert_eq!(plan.execution_ms, Some(133.009));
    // Every loop's rows: 61855.67 per loop, three loops.
    assert!(close(plan.rows_out(3).unwrap(), 61855.67 * 3.0));
    assert!(close(scan.removed.unwrap(), 4811.0 * 3.0));
    assert!(scan.actual.unwrap().rows_decimals, "PostgreSQL 18 writes rows with decimals");
    assert!(!parse(&fixture(17, "parallel.analyze.json")).unwrap().nodes[3].actual.unwrap().rows_decimals);
}

#[test]
fn subplans_are_taken_from_their_parent_and_loops_multiply() {
    let plan = parse(&fixture(18, "subplans.analyze.json")).unwrap();
    let root = &plan.nodes[0];
    let sub = &plan.nodes[find(&plan, "Aggregate")];
    assert_eq!((sub.relationship.as_deref(), sub.subplan.as_deref()), (Some("SubPlan"), Some("SubPlan 1")));
    // 250 loops of 0.319 ms.
    assert!(close(sub.total_ms.unwrap(), 0.319 * 250.0));
    let init = &plan.nodes[find(&plan, "Result")];
    assert_eq!(init.subplan.as_deref(), Some("InitPlan 3"));
    assert!(close(root.self_ms.unwrap(), 80.545 - 0.042 - 0.319 * 250.0), "{:?}", root.self_ms);
}

#[test]
fn a_ctes_time_is_taken_from_the_scans_that_read_it() {
    let plan = parse(&fixture(18, "cte.analyze.json")).unwrap();
    assert_eq!(ops(&plan), ["Hash Join", "HashAggregate", "Seq Scan", "CTE Scan", "Hash", "CTE Scan"]);
    let n = |i: usize| plan.nodes[i].self_ms.unwrap();
    assert_eq!(plan.nodes[1].subplan.as_deref(), Some("CTE big"));
    // The join's own time: not less the CTE (it is inside the scan below it).
    assert!(close(n(0), 171.352 - 170.295 - 0.716), "{}", n(0));
    assert!(close(n(1), 170.518 - 54.908));
    // The first scan read (and so ran) the whole CTE; the rest of its time is taken from the
    // second scan.
    assert!(close(n(3), 0.0), "{}", n(3));
    assert!(close(n(5), 0.539 - (170.518 - 170.295)), "{}", n(5));
    let sum: f64 = (0..plan.nodes.len()).map(n).sum();
    assert!(close(sum, 171.352), "the parts add up to the whole: {sum}");
}

#[test]
fn a_node_that_never_ran_has_no_time_and_no_misestimate() {
    let plan = parse(&fixture(18, "never.analyze.json")).unwrap();
    let inner = find(&plan, "Index Only Scan");
    let a = plan.nodes[inner].actual.unwrap();
    assert_eq!(a.loops, 0.0);
    assert_eq!(plan.nodes[inner].total_ms, Some(0.0));
    assert_eq!(plan.rows_off(inner), None);
    assert!(text::node_line(&plan, inner).ends_with("(never executed)"), "{}", text::node_line(&plan, inner));
    assert_eq!(plan.nodes[0].op, "Nested Loop Semi Join");
}

#[test]
fn triggers_jit_and_settings_are_read() {
    let plan = parse(&fixture(18, "insert.analyze.json")).unwrap();
    assert_eq!(plan.nodes[0].op, "Insert");
    assert_eq!(plan.nodes[0].target.as_deref(), Some("on zz_orders"));
    assert_eq!(plan.triggers.len(), 1);
    assert_eq!((plan.triggers[0].name.as_str(), plan.triggers[0].calls), ("zz_orders_audit", 50.0));
    let footer: Vec<&str> = plan.footer.iter().map(|(l, _)| l.as_str()).collect();
    assert!(footer.contains(&"Trigger zz_orders_audit: time=2.803 calls=50"), "{footer:?}");
    for v in VERSIONS {
        let plan = parse(&fixture(v, "jit.analyze.json")).unwrap();
        let jit = plan.footer.iter().find(|(l, _)| l == "JIT:").unwrap_or_else(|| panic!("pg{v}: {:?}", plan.footer));
        assert!(jit.1.iter().any(|l| l.starts_with("Functions: ")), "pg{v}: {:?}", jit.1);
        assert!(jit.1.iter().any(|l| l.starts_with("Timing: Generation ")), "pg{v}: {:?}", jit.1);
        let verbose = parse(&fixture(v, "join.verbose.json")).unwrap();
        assert!(verbose.footer[0].0.starts_with("Settings: "), "pg{v}: {:?}", verbose.footer);
        assert!(verbose.nodes[0].properties.iter().any(|(k, _)| k == "Output"));
        assert!(verbose.nodes.iter().any(|n| n.target.as_deref().is_some_and(|t| t.starts_with("on public."))));
        assert_eq!(verbose.planning_buffers.is_some(), v >= 13);
    }
    // PostgreSQL 17 splits a timing into its parts.
    let plan = parse(&fixture(18, "jit.analyze.json")).unwrap();
    let jit = &plan.footer.iter().find(|(l, _)| l == "JIT:").unwrap().1;
    assert!(jit.iter().any(|l| l.contains("Generation 0.460 ms (Deform 0.173 ms)")), "{jit:?}");
}

#[test]
fn partitions_and_buffers_read_as_the_text_says_them() {
    let plan = parse(&fixture(16, "partition.analyze.json")).unwrap();
    assert!(plan.nodes.iter().any(|n| n.target.as_deref() == Some("on zz_events_q1 zz_events_1")));
    let buffers = plan.buffers().unwrap();
    assert_eq!(Some(buffers), plan.nodes[0].buffers);
    let line = plan.nodes[0].properties.iter().find(|(k, _)| k == "Buffers").unwrap();
    assert_eq!(line.1, buffers_text(&buffers).unwrap());
    assert!(line.1.starts_with("shared hit="));
}

/// A node as the JSON of the tests writes it.
fn json_node(kind: &str, extra: &str, children: &[String]) -> String {
    let plans = if children.is_empty() { String::new() } else { format!(", \"Plans\": [{}]", children.join(",")) };
    format!(
        r#"{{"Node Type": "{kind}", "Startup Cost": 0.00, "Total Cost": 10.00, "Plan Rows": 10, "Plan Width": 4{extra}{plans}}}"#
    )
}

#[test]
fn rows_off_by_ten_or_more_either_way_is_a_misestimate() {
    let leaf = |rows: &str, loops: &str| {
        json_node(
            "Seq Scan",
            &format!(
                r#", "Relation Name": "t", "Alias": "t", "Actual Startup Time": 0.1, "Actual Total Time": 1.0, "Actual Rows": {rows}, "Actual Loops": {loops}"#
            ),
            &[],
        )
    };
    let doc = |n: String| format!(r#"[{{"Plan": {n}, "Execution Time": 2.0}}]"#);
    let off = |rows: &str| parse(&doc(leaf(rows, "1"))).unwrap().rows_off(0).unwrap();
    // Estimated 10 per loop.
    assert!(close(off("100").ratio, 10.0) && off("100").under);
    assert!(close(off("1").ratio, 10.0) && !off("1").under);
    assert!(close(off("0").ratio, 10.0), "no rows counts as one");
    assert!(close(off("50").ratio, 5.0));
    let plan = parse(&doc(leaf("100", "1"))).unwrap();
    assert!(plan.misestimate(0).is_some());
    let plan = parse(&doc(leaf("99", "1"))).unwrap();
    assert!(plan.misestimate(0).is_none());
    // Per loop: 100 loops of 10 rows is as estimated.
    let plan = parse(&doc(leaf("10", "100"))).unwrap();
    assert!(plan.misestimate(0).is_none());
    assert!(close(plan.rows_out(0).unwrap(), 1000.0));
    assert!(close(plan.nodes[0].total_ms.unwrap(), 100.0), "a loop each");
}

#[test]
fn a_node_is_hot_from_a_fifth_of_the_execution_time() {
    let timed = |kind: &str, total: f64, children: &[String]| {
        json_node(
            kind,
            &format!(
                r#", "Actual Startup Time": 0.0, "Actual Total Time": {total}, "Actual Rows": 1, "Actual Loops": 1"#
            ),
            children,
        )
    };
    let a = timed("Seq Scan", 20.0, &[]);
    let b = timed("Seq Scan", 19.0, &[]);
    let root = timed("Append", 100.0, &[a, b]);
    let plan = parse(&format!(r#"[{{"Plan": {root}, "Execution Time": 100.0}}]"#)).unwrap();
    assert!(close(plan.weight(0), 61.0));
    assert!(plan.is_hot(0) && plan.is_hot(1) && !plan.is_hot(2));
    assert_eq!(plan.slowest(), Some(0));
    // Without ANALYZE: by cost (10 each, the root's own 0), never as time.
    let plan = parse(&format!(
        r#"[{{"Plan": {}}}]"#,
        json_node("Append", "", &[json_node("Seq Scan", "", &[]), json_node("Seq Scan", "", &[])])
    ))
    .unwrap();
    assert_eq!(plan.measure(), Measure::Cost);
    assert!(close(plan.total(), 10.0) && close(plan.weight(1), 10.0) && close(plan.weight(0), 0.0));
}

#[test]
fn unknown_keys_are_kept_as_properties_and_never_an_error() {
    let n = json_node(
        "Future Scan",
        r#", "Relation Name": "t", "Shiny New Counter": 7, "Brand New Flag": true, "Off Flag": false, "Nested": {"A": 1, "B": "x"}"#,
        &[],
    );
    let plan = parse(&format!(r#"[{{"Plan": {n}, "Some Summary": 3.5}}]"#)).unwrap();
    let props = &plan.nodes[0].properties;
    assert!(props.contains(&("Shiny New Counter".into(), "7".into())), "{props:?}");
    assert!(props.contains(&("Brand New Flag".into(), "true".into())));
    assert!(props.contains(&("Nested".into(), "A: 1  B: x".into())));
    assert!(!props.iter().any(|(k, _)| k == "Off Flag"), "a false flag says nothing");
    assert_eq!(plan.footer, [("Some Summary: 3.5".to_string(), Vec::new())]);
    assert_eq!(plan.nodes[0].target.as_deref(), Some("on t"));
}

#[test]
fn what_is_not_a_plan_says_so() {
    assert_eq!(parse("not json"), Err(PlanError::Json));
    assert_eq!(parse("[1, 2]"), Err(PlanError::NotAPlan));
    assert_eq!(parse(r#"[{"Plan": 1}]"#), Err(PlanError::NotAPlan));
    assert_eq!(parse(r#"{"Plan": {"No": 1}}"#), Err(PlanError::NotAPlan));
    assert_eq!(parse("[]"), Err(PlanError::NotAPlan));
    assert!(parse(r#"{"Plan": {"Node Type": "Result"}}"#).is_ok(), "one object is a plan too");
}

#[test]
fn a_very_deep_plan_reads_without_recursion() {
    let depth = 20_000;
    let mut s = String::from("[{\"Plan\": ");
    for _ in 0..depth {
        s.push_str(r#"{"Node Type": "Limit", "Plans": ["#);
    }
    s.push_str(r#"{"Node Type": "Result"}"#);
    for _ in 0..depth {
        s.push_str("]}");
    }
    s.push_str("}]");
    let plan = parse(&s).unwrap();
    assert_eq!(plan.nodes.len(), depth + 1);
    assert_eq!(plan.nodes[depth].depth, depth);
    assert_eq!(text::lines(&plan).len(), depth + 1);
}

#[test]
fn several_plans_read_the_first_and_count_the_rest() {
    let one = json_node("Result", "", &[]);
    let plan = parse(&format!(r#"[{{"Plan": {one}}}, {{"Plan": {one}}}]"#)).unwrap();
    assert_eq!((plan.nodes.len(), plan.more_plans), (1, 1));
}

#[test]
fn names_are_quoted_as_the_server_quotes_them() {
    let n = json_node(
        "Index Scan",
        r#", "Index Name": "Idx X", "Relation Name": "Order", "Alias": "o", "Scan Direction": "Backward""#,
        &[],
    );
    let plan = parse(&format!(r#"{{"Plan": {n}}}"#)).unwrap();
    assert_eq!(plan.label(0), r#"Index Scan Backward using "Idx X" on "Order" o"#);
    let n = json_node("Hash Join", r#", "Join Type": "Right Anti", "Parallel Aware": true"#, &[]);
    assert_eq!(parse(&format!(r#"{{"Plan": {n}}}"#)).unwrap().nodes[0].op, "Parallel Hash Right Anti Join");
    let n = json_node("Nested Loop", r#", "Join Type": "Left""#, &[]);
    assert_eq!(parse(&format!(r#"{{"Plan": {n}}}"#)).unwrap().nodes[0].op, "Nested Loop Left Join");
}

#[test]
fn the_plan_column_is_recognised() {
    assert!(is_plan_column("QUERY PLAN", "json"));
    assert!(!is_plan_column("QUERY PLAN", "text"), "the text form is not read");
    assert!(!is_plan_column("plan", "json"));
}
