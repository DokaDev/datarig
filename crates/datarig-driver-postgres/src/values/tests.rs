use super::*;

fn num(ndigits: i16, weight: i16, sign: u16, dscale: u16, digits: &[i16]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend(ndigits.to_be_bytes());
    v.extend(weight.to_be_bytes());
    v.extend(sign.to_be_bytes());
    v.extend(dscale.to_be_bytes());
    for d in digits {
        v.extend(d.to_be_bytes());
    }
    v
}

#[test]
fn numeric_decoding() {
    assert_eq!(numeric_to_string(&num(2, 0, 0, 2, &[1234, 5000])).unwrap(), "1234.50");
    assert_eq!(numeric_to_string(&num(1, -1, 0, 2, &[100])).unwrap(), "0.01");
    assert_eq!(numeric_to_string(&num(2, 1, 0x4000, 0, &[12, 3456])).unwrap(), "-123456");
    assert_eq!(numeric_to_string(&num(0, 0, 0, 0, &[])).unwrap(), "0");
    assert_eq!(numeric_to_string(&num(0, 0, 0, 2, &[])).unwrap(), "0.00");
    assert_eq!(numeric_to_string(&num(1, 2, 0, 0, &[7])).unwrap(), "700000000");
    assert_eq!(numeric_to_string(&num(1, -2, 0, 6, &[5000])).unwrap(), "0.000050");
    assert_eq!(numeric_to_string(&num(0, 0, 0xC000, 0, &[])).unwrap(), "NaN");
}

#[test]
fn scalar_formats() {
    assert_eq!(format_value(&Type::INT8, &42i64.to_be_bytes(), false), "42");
    assert_eq!(format_value(&Type::BOOL, &[1], false), "true");
    assert_eq!(format_value(&Type::TEXT, "漢字".as_bytes(), false), "漢字");
    assert_eq!(format_value(&Type::JSONB, b"\x01{\"a\":1}", false), "{\"a\":1}");
    assert_eq!(format_value(&Type::BYTEA, &[0xde, 0xad], false), "\\xdead");
    assert_eq!(format_value(&Type::TIME, &3_723_500_000i64.to_be_bytes(), false), "01:02:03.5");
    let mut iv = Vec::new();
    iv.extend(3_600_000_000i64.to_be_bytes());
    iv.extend(2i32.to_be_bytes());
    iv.extend(14i32.to_be_bytes());
    assert_eq!(format_value(&Type::INTERVAL, &iv, false), "1 year 2 mons 2 days 01:00:00");
}

#[test]
fn types_without_a_binary_decoder_are_fetched_as_text() {
    for ty in [Type::INT8, Type::NUMERIC, Type::TIMESTAMPTZ, Type::DATE, Type::JSONB, Type::INT4_ARRAY, Type::TEXT] {
        assert_eq!(format_code(&ty), 1, "{ty}");
    }
    for ty in [
        Type::BIT,
        Type::VARBIT,
        Type::INT4_RANGE,
        Type::INT4MULTI_RANGE,
        Type::POINT,
        Type::LINE,
        Type::BOX,
        Type::CIRCLE,
        Type::POLYGON,
        Type::PATH,
        Type::TS_VECTOR,
        Type::TSQUERY,
        Type::MACADDR,
        Type::MACADDR8,
        Type::MONEY,
        Type::CHAR,
        Type::REGCLASS,
        Type::PG_LSN,
        Type::XML,
        Type::POINT_ARRAY,
        Type::TSTZ_RANGE_ARRAY,
    ] {
        assert_eq!(format_code(&ty), 0, "{ty}");
    }
    // An unknown type (an extension's) is text too, and text is passed on as it is.
    let unknown = Type::new("hstore".into(), 99_999, Kind::Simple, "public".into());
    assert_eq!(format_code(&unknown), 0);
    assert_eq!(format_value(&unknown, b"\"a\"=>\"1\"", true), "\"a\"=>\"1\"");
    assert_eq!(format_value(&Type::BIT, b"1010", true), "1010", "never the binary bytes");
}

#[test]
fn dates_and_timestamps_keep_infinity_bc_and_far_years() {
    let date = |days: i32| format_value(&Type::DATE, &days.to_be_bytes(), false);
    let ts = |micros: i64| format_value(&Type::TIMESTAMP, &micros.to_be_bytes(), false);
    assert_eq!(date(0), "2000-01-01");
    assert_eq!(date(i32::MAX), "infinity");
    assert_eq!(date(i32::MIN), "-infinity");
    // 0044-03-15 BC is year -43; 0001-01-01 BC is year 0.
    assert_eq!(date(-746_117), "0044-03-15 BC");
    assert_eq!(date(-730_485), "0001-01-01 BC");
    assert_eq!(date(-730_119), "0001-01-01");
    assert_eq!(date(2_914_635), "9980-01-01");
    assert_eq!(date(5_000_151), "15689-12-12", "beyond four digits");
    assert_eq!(ts(i64::MAX), "infinity");
    assert_eq!(ts(i64::MIN), "-infinity");
    assert_eq!(ts(-746_117 * 86_400_000_000 + 43_200_500_000), "0044-03-15 12:00:00.5 BC");
    assert_eq!(
        ts(106_751_982 * 86_400_000_000 + 86_399_999_999),
        "294276-12-31 23:59:59.999999",
        "the last day PostgreSQL stores"
    );
    assert_eq!(ts(-1), "1999-12-31 23:59:59.999999");
}

#[test]
fn offsets_and_floats_are_written_as_postgres_writes_them() {
    assert_eq!(fmt_offset(9 * 3600), "+09");
    assert_eq!(fmt_offset(-(3 * 3600 + 1800)), "-03:30");
    assert_eq!(fmt_offset(8 * 3600 + 27 * 60 + 52), "+08:27:52", "local mean time keeps its seconds");
    assert_eq!(fmt_timestamp(0, Some(0)), "2000-01-01 00:00:00+00");
    assert_eq!(fmt_timestamp(-746_117 * 86_400_000_000, Some(0)), "0044-03-15 00:00:00+00 BC");
    assert_eq!(format_value(&Type::FLOAT8, &f64::INFINITY.to_be_bytes(), false), "Infinity");
    assert_eq!(format_value(&Type::FLOAT8, &f64::NEG_INFINITY.to_be_bytes(), false), "-Infinity");
    assert_eq!(format_value(&Type::FLOAT4, &f32::NAN.to_be_bytes(), false), "NaN");
    assert_eq!(format_value(&Type::FLOAT8, &0.1f64.to_be_bytes(), false), "0.1");
}

#[test]
fn floats_are_the_servers_shortest_text() {
    let f8 = |v: f64| format_value(&Type::FLOAT8, &v.to_be_bytes(), false);
    let f4 = |v: f32| format_value(&Type::FLOAT4, &v.to_be_bytes(), false);
    // What PostgreSQL writes for these (`extra_float_digits = 1`).
    for (v, want) in [
        (f64::MAX, "1.7976931348623157e+308"),
        (-f64::MAX, "-1.7976931348623157e+308"),
        (5e-324, "5e-324"),
        (f64::MIN_POSITIVE, "2.2250738585072014e-308"),
        (1e15, "1e+15"),
        (123456789012345.0, "123456789012345"),
        (1e22, "1e+22"),
        (100.0, "100"),
        (0.1, "0.1"),
        (0.0001, "0.0001"),
        (0.00001, "1e-05"),
        (-1.5e-7, "-1.5e-07"),
        (1.5, "1.5"),
        (0.0, "0"),
        (-0.0, "-0"),
    ] {
        assert_eq!(f8(v), want, "{v:e}");
    }
    for (v, want) in [
        (f32::MAX, "3.4028235e+38"),
        (1e-45, "1e-45"),
        (f32::MIN_POSITIVE, "1.1754944e-38"),
        (123456.0, "123456"),
        (1234567.0, "1.234567e+06"),
        (1e6, "1e+06"),
        (100000.0, "100000"),
        (0.1, "0.1"),
        (0.00001, "1e-05"),
        (-0.0, "-0"),
    ] {
        assert_eq!(f4(v), want, "{v:e}");
    }
}

/// The binary form of an int4 array with `dims` (length, lower bound) and `values`.
fn int4_array(dims: &[(i32, i32)], values: &[Option<i32>]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend((dims.len() as i32).to_be_bytes());
    v.extend(i32::from(values.contains(&None)).to_be_bytes());
    v.extend(23u32.to_be_bytes());
    for (len, lower) in dims {
        v.extend(len.to_be_bytes());
        v.extend(lower.to_be_bytes());
    }
    for x in values {
        match x {
            None => v.extend((-1i32).to_be_bytes()),
            Some(x) => {
                v.extend(4i32.to_be_bytes());
                v.extend(x.to_be_bytes());
            }
        }
    }
    v
}

#[test]
fn arrays_keep_their_dimensions_and_bounds() {
    let arr =
        |dims: &[(i32, i32)], values: &[Option<i32>]| format_value(&Type::INT4_ARRAY, &int4_array(dims, values), false);
    assert_eq!(arr(&[(3, 1)], &[Some(1), None, Some(3)]), "{1,NULL,3}");
    assert_eq!(arr(&[(2, 1), (2, 1)], &[Some(1), Some(2), Some(3), Some(4)]), "{{1,2},{3,4}}");
    assert_eq!(arr(&[(2, 1), (1, 1), (2, 1)], &[Some(1), Some(2), Some(3), Some(4)]), "{{{1,2}},{{3,4}}}");
    assert_eq!(arr(&[(2, 0)], &[Some(7), Some(8)]), "[0:1]={7,8}");
    assert_eq!(arr(&[(1, -2), (2, 1)], &[Some(7), Some(8)]), "[-2:-2][1:2]={{7,8}}");
    assert_eq!(arr(&[], &[]), "{}");
}

/// Every type a column can have here: each built-in type, a domain over each and an array of
/// that domain, and types of a database's own named like built-in ones.
fn every_type() -> Vec<Type> {
    let builtin: Vec<Type> = (0..=u32::from(u16::MAX)).filter_map(Type::from_oid).collect();
    let mut out = builtin.clone();
    for (i, ty) in builtin.into_iter().enumerate() {
        let n = 1_000_000 + 2 * i as u32;
        let domain = Type::new(format!("d{n}"), n, Kind::Domain(ty), "public".into());
        out.push(Type::new(format!("_d{n}"), n + 1, Kind::Array(domain.clone()), "public".into()));
        out.push(domain);
    }
    // A database's own types named like built-in ones, or with their names in theirs: an enum,
    // a composite and a domain (over text, over a number, over a timestamp) of each name, and an
    // array of each.
    let names = [
        "timestamp_kind",
        "My_Timestamp",
        "my_datetime",
        "date",
        "time",
        "timetz",
        "time of day",
        "bool",
        "boolean",
        "json",
        "jsonb",
        "int4",
        "numeric",
        "x[]",
        "mood",
    ];
    let mut n = 2_000_000;
    for name in names {
        for kind in [
            Kind::Enum(vec!["a".into()]),
            Kind::Composite(Vec::new()),
            Kind::Domain(Type::TEXT),
            Kind::Domain(Type::INT4),
            Kind::Domain(Type::TIMESTAMPTZ),
        ] {
            let ty = Type::new(name.into(), n, kind, "public".into());
            out.push(Type::new(format!("_{name}"), n + 1, Kind::Array(ty.clone()), "public".into()));
            out.push(ty);
            n += 2;
        }
    }
    out
}

fn meta_of(ty: &Type) -> datarig_core::driver::ColumnMeta {
    datarig_core::driver::ColumnMeta {
        name: "c".into(),
        type_name: type_display(ty),
        numeric: is_numeric(ty),
        json: is_json(ty),
        kind: value_kind(ty),
        origin: None,
    }
}

/// The copy writes every column as it did when it read the type's name: its kind of a column
/// (`Kind::of_column`) is [`export::Kind::of`] over the column's type name and flags.
#[test]
fn the_copy_kind_of_every_type_is_what_its_name_said() {
    use datarig_core::export::Kind as CopyKind;
    for ty in every_type() {
        let m = meta_of(&ty);
        assert_eq!(CopyKind::of_column(&m), CopyKind::of(&m.type_name, m.numeric, m.json), "{}", m.type_name);
        assert_eq!(m.numeric, m.kind.is_number(), "{}", m.type_name);
        assert_eq!(m.json, m.kind == ValueKind::Json, "{}", m.type_name);
    }
}

/// The chart gives every column the role it gave when it read the type's name: the role from
/// the name (or, when the name says nothing, from the values) equals the role from the kind.
#[test]
fn the_chart_role_of_every_type_is_what_its_name_said() {
    use datarig_core::chart::{Role, TimeKind, roles};
    // The role as the chart told it from the type's name, `None` when it looked at the values.
    let by_name = |m: &datarig_core::driver::ColumnMeta| {
        let t = m.type_name.to_ascii_lowercase();
        if m.json || t.ends_with("[]") {
            Some(Role::Unusable)
        } else if m.numeric {
            Some(Role::Number)
        } else if t.contains("timestamp") || t.contains("datetime") {
            Some(Role::Time(TimeKind::DateTime))
        } else if t == "date" {
            Some(Role::Time(TimeKind::Date))
        } else if t == "time" || t == "timetz" || t.starts_with("time ") {
            Some(Role::Time(TimeKind::Time))
        } else {
            None
        }
    };
    let samples: [Vec<Vec<Option<String>>>; 3] = [
        vec![vec![Some("2024-02-29".into())]],
        vec![vec![Some("2024-02-29 12:00:00".into())]],
        vec![vec![Some("x".into())]],
    ];
    let mut kinds = std::collections::HashSet::new();
    for ty in every_type() {
        let m = meta_of(&ty);
        kinds.insert(m.kind);
        for sample in &samples {
            let from_values = datarig_core::driver::ColumnMeta { kind: ValueKind::Text, ..m.clone() };
            let want = by_name(&m).unwrap_or_else(|| roles(&[from_values], sample)[0]);
            assert_eq!(roles(std::slice::from_ref(&m), sample)[0], want, "{} {sample:?}", m.type_name);
        }
    }
    assert!(kinds.len() >= 15, "the types reach every kind: {kinds:?}");
}
