use super::short_type;
use datarig_core::sql::dialect::Dialect;

#[test]
fn postgresql_types_are_listed_by_their_short_names() {
    let pg = |t: &str| short_type(Dialect::Postgres, t);
    assert_eq!(pg("timestamp with time zone"), "timestamptz");
    assert_eq!(pg("timestamp without time zone"), "timestamp");
    assert_eq!(pg("timestamp(3) with time zone"), "timestamptz(3)");
    assert_eq!(pg("time without time zone[]"), "time[]");
    assert_eq!(pg("time(6) with time zone[][]"), "timetz(6)[][]");
    assert_eq!(pg("character varying(20)"), "varchar(20)");
    assert_eq!(pg("character varying"), "varchar");
    assert_eq!(pg("character(3)[]"), "char(3)[]");
    assert_eq!(pg("double precision"), "float8");
    assert_eq!(pg("bit varying(8)"), "varbit(8)");
    // Names that only start like one stay as written.
    for t in ["timestamptz", "numeric(14,2)", "characters", "time_slot", "\"character\"", "interval", "text[]"] {
        assert_eq!(pg(t), t);
    }
    // MySQL's names are short already.
    assert_eq!(short_type(Dialect::MySql(Default::default()), "character varying(20)"), "character varying(20)");
}
