use super::*;

#[test]
fn median_and_p95_by_nearest_rank() {
    let s = Summary::of(&[5.0, 1.0, 3.0, 2.0, 4.0]);
    assert_eq!((s.median, s.p95, s.min, s.max), (3.0, 5.0, 1.0, 5.0));
    let xs: Vec<f64> = (1..=100).map(f64::from).collect();
    let s = Summary::of(&xs);
    assert_eq!((s.median, s.p95), (50.5, 95.0));
    assert_eq!(Summary::of(&[]).n, 0);
}

#[test]
fn ps_cpu_time() {
    assert_eq!(parse_cpu_time("0:00.12"), Some(0.12));
    assert_eq!(parse_cpu_time("1:02:03"), Some(3723.0));
    assert_eq!(parse_cpu_time("2-00:00:01"), Some(172_801.0));
    assert_eq!(parse_cpu_time("x"), None);
}
