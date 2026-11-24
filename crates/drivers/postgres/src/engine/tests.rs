use super::*;

/// The edits the driver offers every session are the ones PostgreSQL's edit rules answer, on
/// a server of any version.
#[test]
fn the_static_postgres_edit_is_the_specs() {
    let Fact::Known(rules) = EngineId::Postgres.spec().edit else { panic!("PostgreSQL takes edits") };
    for version in
        [Fact::Known(Version::new(17, 2, 0)), Fact::Known(Version::new(9, 6, 0)), Fact::Unknown(Why::VersionUnknown)]
    {
        let f = ServerFacts {
            version: version.clone(),
            version_text: None,
            provider: Fact::Unknown(Why::NotChecked),
            topology: Fact::Known(Topology::Direct),
            detail: EngineFacts::Postgres(PgFacts::default()),
        };
        assert_eq!(rules.writer(&f), Fact::Known(EditFormat::Postgres), "{version:?}");
        let answer = match rules.writer(&f) {
            Fact::Known(format) => Some(format),
            Fact::No(_) | Fact::Unknown(_) => None,
        };
        assert_eq!(CAPABILITIES.edit, answer, "{version:?}");
    }
}
