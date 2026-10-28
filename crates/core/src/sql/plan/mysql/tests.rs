//! The fixtures are tree text captured from MySQL 8.0.45, 8.4.11 and 9.7.4
//! (`fixtures/mysql/<version>/`): the tables of `fixtures/mysql/setup.sql`, each statement of
//! `fixtures/mysql/queries.txt` as `<name>.tree.txt` (`EXPLAIN FORMAT=TREE`) and
//! `<name>.analyze.txt` (`EXPLAIN ANALYZE`). They cover joins (nested loop, hash), a semi and an
//! anti join, a subquery run once and one per row, a union, a recursive CTE, a window, a derived
//! table, a node that never ran and writes (some of which the server has no tree for).

use super::*;
use crate::sql::plan::Measure;

const VERSIONS: [&str; 3] = ["8.0", "8.4", "9.7"];

fn fixture(version: &str, name: &str) -> String {
    let path = format!("{}/src/sql/plan/fixtures/mysql/{version}/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn names() -> Vec<String> {
    let path = format!("{}/src/sql/plan/fixtures/mysql/queries.txt", env!("CARGO_MANIFEST_DIR"));
    let q = std::fs::read_to_string(path).unwrap();
    q.lines().filter_map(|l| l.split_once('|').map(|(n, _)| n.to_string())).collect()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9 * a.abs().max(1.0)
}

/// Every fixture is a plan, except those whose statement the server has no tree for.
#[test]
fn every_fixture_of_every_version_is_a_plan() {
    for v in VERSIONS {
        for name in names() {
            for (file, analyzed) in [(format!("{name}.analyze.txt"), true), (format!("{name}.tree.txt"), false)] {
                let text = fixture(v, &file);
                // 8.0 writes it without `-> `, later versions as a node.
                if text.contains("<not executable by iterator executor>") {
                    assert_eq!(parse(&text), Err(PlanError::NotAPlan), "mysql{v} {file}");
                    continue;
                }
                assert!(is_tree(&text), "mysql{v} {file}");
                let plan = parse(&text).unwrap_or_else(|e| panic!("mysql{v} {file}: {e:?}"));
                // A subquery in the select list is a tree of its own after the query's.
                let roots = if name == "scalar_sub" { 2 } else { 1 };
                assert_eq!(plan.roots().count(), roots, "mysql{v} {file}");
                // An INSERT's tree has no measures at all.
                let measured = plan.nodes.iter().any(|n| n.actual.is_some());
                assert_eq!(plan.analyzed, measured, "mysql{v} {file}");
                assert!(analyzed || !measured, "mysql{v} {file}");
                if measured {
                    assert_eq!(plan.measure(), Measure::Time, "mysql{v} {file}");
                }
                for (i, n) in plan.nodes.iter().enumerate() {
                    assert!(!n.op.is_empty(), "mysql{v} {file} node {i}");
                    assert!(!n.op.contains("(cost=") && !n.op.contains("(actual"), "mysql{v} {file}: {}", n.op);
                    if let Some(p) = n.parent {
                        assert!(p < i && plan.nodes[p].children.contains(&i), "depth first, parent first");
                        assert_eq!(n.depth, plan.nodes[p].depth + 1);
                    }
                    if measured {
                        assert!(n.self_ms.is_some_and(|s| s >= 0.0), "mysql{v} {file} node {i}");
                    }
                }
                // Self times add up to the roots' totals.
                if measured {
                    let selfs: f64 = plan.nodes.iter().filter_map(|n| n.self_ms).sum();
                    let roots: f64 = plan.roots().filter_map(|r| plan.nodes[r].total_ms).sum();
                    assert!(selfs + 1e-9 >= roots, "mysql{v} {file}: {selfs} < {roots}");
                }
            }
        }
    }
}

/// The join of the design note, read on 8.4: operations, targets, estimates and measures.
#[test]
fn a_join_reads_its_numbers_and_names() {
    let plan = parse(&fixture("8.4", "join_group_sort.analyze.txt")).unwrap();
    let ops: Vec<&str> = plan.nodes.iter().map(|n| n.op.as_str()).collect();
    assert_eq!(
        ops,
        [
            "Sort",
            "Stream results",
            "Group aggregate",
            "Nested loop inner join",
            "Filter",
            "Covering index range scan",
            "Index lookup"
        ]
    );
    let lookup = &plan.nodes[6];
    assert_eq!(lookup.target.as_deref(), Some("on o using c_id (c_id=c.id)"));
    assert_eq!(lookup.cost, Some((2.52, 2.52)));
    assert_eq!(lookup.plan_rows, Some(10.0));
    let a = lookup.actual.unwrap();
    assert_eq!((a.startup_ms, a.total_ms, a.rows, a.loops), (Some(0.0131), Some(0.0143), 10.0, 58.0));
    assert!(close(lookup.total_ms.unwrap(), 0.0143 * 58.0));
    assert_eq!(plan.nodes[0].properties[0], ("Sort".to_string(), "`SUM(o.amount)` DESC".to_string()));
    assert_eq!(plan.nodes[0].cost, Some((0.0, 273.0)), "no cost of its own: its child's");
    assert!(plan.nodes[0].cost_derived && !plan.nodes[1].cost_derived);
    assert_eq!(plan.nodes[4].properties[0].1, "(c.city in ('city1','city2'))");
}

/// Numbers in every form the server writes them: fractional rows, exponents, a cost range.
#[test]
fn numbers_in_every_form_are_read() {
    let n = node("Rows fetched before execution  (cost=0..0 rows=1) (actual time=82e-6..123e-6 rows=1 loops=1)");
    assert_eq!(n.cost, Some((0.0, 0.0)));
    assert_eq!(n.actual.unwrap().startup_ms, Some(82e-6));
    let n = node("Filter: (x)  (cost=268..268 rows=1) (actual time=0.0266..0.0266 rows=0.116 loops=43)");
    let a = n.actual.unwrap();
    assert!(a.rows_decimals && a.rows == 0.116 && a.loops == 43.0);
    let n = node("Index lookup on o using c_id (c_id=c.id)  (cost=2.52 rows=10) (never executed)");
    assert_eq!(n.actual.map(|a| a.loops), Some(0.0));
    assert_eq!(n.cost, Some((2.52, 2.52)));
}

/// A string literal in a condition holds whatever it holds: it can't add a node, a measure or
/// a cost; the groups are read from the end of the line only.
#[test]
fn a_literal_cannot_forge_nodes_or_numbers() {
    let text = "-> Filter: (c.`name` = 'a\\n    -> Fake node  (cost=1 rows=1) (actual time=1..2 rows=3 loops=1)')  \
                (cost=20.2 rows=200) (actual time=1.82..1.83 rows=5 loops=1)\n    -> Table scan on c  (cost=20.2 rows=200) \
                (actual time=0.027..0.0414 rows=42 loops=1)\n";
    let plan = parse(text).unwrap();
    assert_eq!(plan.nodes.len(), 2);
    assert_eq!(plan.nodes[0].plan_rows, Some(200.0));
    assert!(plan.nodes[0].properties[0].1.contains("Fake node  (cost=1 rows=1)"));
    // A node without groups whose text ends like one keeps it as text.
    let n = node("Sort: f('x  (cost=1 rows=2)')");
    assert_eq!((n.cost, n.plan_rows), (None, None));
    let n = node("Sort: `a  (cost=1 rows=2)`");
    assert_eq!((n.cost, n.plan_rows), (None, None));
    let n = node("Sort: x  (cost=1e400 rows=2)");
    assert_eq!(n.cost, None, "not a finite number");
}

/// Text that is not tree text, or a tree too odd to trust, is no plan.
#[test]
fn what_is_not_a_tree_is_refused() {
    for bad in ["", "   ", "id\tselect_type", "    -> Table scan on c", "-> A\n        -> B", "-> A\n  -> B", "x\n-> A"]
    {
        assert_eq!(parse(bad), Err(PlanError::NotAPlan), "{bad:?}");
    }
    assert!(!is_tree("{\"query\": 1}") && !is_tree("") && is_tree("\n-> Table scan on c"));
    let deep: String = (0..super::super::MAX_NODES + 1).map(|_| "-> Hash\n").collect();
    assert_eq!(parse(&deep), Err(PlanError::TooBig));
}

/// A node the server writes no measure for holds what is under it; the plan is still timed.
#[test]
fn unmeasured_nodes_hold_their_children() {
    let plan = parse(&fixture("8.0", "hash_join.analyze.txt")).unwrap();
    let hash = plan.nodes.iter().position(|n| n.op == "Hash").unwrap();
    let child = plan.nodes[hash].children[0];
    assert_eq!(plan.nodes[hash].total_ms, plan.nodes[child].total_ms);
    assert_eq!(plan.nodes[hash].self_ms, Some(0.0));
    assert_eq!(plan.measure(), Measure::Time);
}

/// Under a limit and on the inner side of a semi join, fewer rows than estimated is no
/// misestimate.
#[test]
fn early_stops_are_marked() {
    let plan = parse(&fixture("8.4", "subquery_in.analyze.txt")).unwrap();
    let limit = plan.nodes.iter().position(|n| n.op.starts_with("Limit")).unwrap();
    for &c in &plan.nodes[limit].children {
        assert!(plan.nodes[c].early_stop);
    }
}

/// The format seam: how MySQL's tree is asked for, told in a result and read.
#[test]
fn the_tree_format_asks_tells_and_reads() {
    use crate::driver::ExplainFormat::{MySqlTree, PostgresJson};
    assert_eq!(MySqlTree.explain_sql("SELECT 1", false), "EXPLAIN FORMAT=TREE SELECT 1");
    assert_eq!(MySqlTree.explain_sql("SELECT 1", true), "EXPLAIN ANALYZE SELECT 1");
    assert_eq!(PostgresJson.explain_sql("SELECT 1", false), "EXPLAIN (FORMAT JSON) SELECT 1");
    let text = fixture("9.7", "scan.analyze.txt");
    assert!(MySqlTree.holds("EXPLAIN", "TEXT", &text));
    assert!(!MySqlTree.holds("QUERY PLAN", "TEXT", &text) && !MySqlTree.holds("EXPLAIN", "TEXT", "id"));
    assert!(!PostgresJson.holds("EXPLAIN", "TEXT", &text));
    assert_eq!(MySqlTree.read(&text), parse(&text));
    assert!(MySqlTree.is_text() && !PostgresJson.is_text());
}

/// A subquery in the select list is a second tree: it runs for each row the first one gives,
/// outside its time, so the whole is both together.
#[test]
fn a_select_list_subquery_is_a_tree_of_its_own() {
    let plan = parse(&fixture("8.4", "scalar_sub.analyze.txt")).unwrap();
    let roots: Vec<usize> = plan.roots().collect();
    assert_eq!(roots.len(), 2);
    let sub = &plan.nodes[roots[1]];
    assert_eq!(sub.subplan.as_deref(), Some("Select #2"));
    assert_eq!(sub.relationship.as_deref(), Some("subquery in projection"));
    let both: f64 = roots.iter().filter_map(|&r| plan.nodes[r].total_ms).sum();
    assert!(close(plan.total(), both));
}
