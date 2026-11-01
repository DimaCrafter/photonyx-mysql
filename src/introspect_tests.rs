use crate::introspect::{column_default, column_type, string_literal};
use photonyx_sql_shared::schema::*;

#[test]
fn column_types() {
    let int = |size: IntSize, unsigned: bool| ColumnType::Int { size, unsigned };

    // MariaDB keeps the display width, MySQL 8.0.19+ does not
    for (mariadb, mysql, expected) in [
        ("int(11)", "int", int(IntSize::Normal, false)),
        (
            "int(10) unsigned",
            "int unsigned",
            int(IntSize::Normal, true),
        ),
        ("tinyint(4)", "tinyint", int(IntSize::Tiny, false)),
        (
            "tinyint(3) unsigned",
            "tinyint unsigned",
            int(IntSize::Tiny, true),
        ),
        ("smallint(6)", "smallint", int(IntSize::Small, false)),
        ("mediumint(9)", "mediumint", int(IntSize::Medium, false)),
        (
            "bigint(20) unsigned",
            "bigint unsigned",
            int(IntSize::Big, true),
        ),
        ("tinyint(1)", "tinyint(1)", ColumnType::Bool),
        ("float", "float", ColumnType::Float),
        ("double", "double", ColumnType::Double),
        ("varchar(64)", "varchar(64)", ColumnType::Varchar(64)),
        ("char(3)", "char(3)", ColumnType::Char(3)),
        ("text", "text", ColumnType::Text),
        ("datetime", "datetime", ColumnType::DateTime),
    ] {
        assert_eq!(column_type(mariadb), expected, "{mariadb}");
        assert_eq!(column_type(mysql), expected, "{mysql}");
    }

    assert_eq!(column_type("json"), ColumnType::Json);
    assert_eq!(
        column_type("enum('a','b''c','d,e')"),
        ColumnType::Enum(vec!["a".to_owned(), "b'c".to_owned(), "d,e".to_owned()])
    );

    // what the format does not have
    for raw in [
        "decimal(10,2)",
        "datetime(3)",
        "timestamp",
        "longtext",
        "float(7,2)",
        "int(10) unsigned zerofill",
        "tinyint(1) unsigned",
        "set('a')",
    ] {
        let expected = if raw == "tinyint(1) unsigned" {
            int(IntSize::Tiny, true)
        } else {
            ColumnType::Other(raw.to_owned())
        };
        assert_eq!(column_type(raw), expected, "{raw}");
    }
}

#[test]
fn literals() {
    assert_eq!(
        string_literal("'it''s \\\\ ok' rest"),
        Some(("it's \\ ok".to_owned(), " rest"))
    );
    assert_eq!(string_literal("'x\\'y'"), Some(("x'y".to_owned(), "")));
    assert_eq!(string_literal("''"), Some((String::new(), "")));
    assert_eq!(string_literal("'open"), None);
    assert_eq!(string_literal("0"), None);
}

#[test]
fn defaults_are_the_same_on_both_servers() {
    let text = ColumnType::Varchar(20);
    let s = |value: &str| Some(DefaultValue::Str(value.to_owned()));

    // (type, MariaDB COLUMN_DEFAULT, MySQL COLUMN_DEFAULT, MySQL EXTRA, expected)
    let cases: Vec<(
        ColumnType,
        Option<&str>,
        Option<&str>,
        &str,
        Option<DefaultValue>,
    )> = vec![
        (
            text.clone(),
            Some("'it''s \\\\ ok'"),
            Some("it's \\ ok"),
            "",
            s("it's \\ ok"),
        ),
        (text.clone(), Some("''"), Some(""), "", s("")),
        // nullable without a default: the string NULL in MariaDB, SQL NULL in MySQL
        (text.clone(), Some("NULL"), None, "", None),
        // the string 'NULL' as a default
        (text.clone(), Some("'NULL'"), Some("NULL"), "", s("NULL")),
        // NOT NULL without a default
        (text.clone(), None, None, "", None),
        (
            ColumnType::Text,
            Some("''"),
            Some("_utf8mb4\\'\\'"),
            "DEFAULT_GENERATED",
            s(""),
        ),
        (
            ColumnType::Text,
            Some("'x\\'y'"),
            Some("_utf8mb4\\'x\\\\\\'y\\'"),
            "DEFAULT_GENERATED",
            s("x'y"),
        ),
        (
            ColumnType::Enum(vec!["a".to_owned()]),
            Some("'a'"),
            Some("a"),
            "",
            s("a"),
        ),
        (
            ColumnType::Float,
            Some("2"),
            Some("2"),
            "",
            Some(DefaultValue::Float(2.0)),
        ),
        (
            ColumnType::Double,
            Some("0.5"),
            Some("0.5"),
            "",
            Some(DefaultValue::Float(0.5)),
        ),
        (
            ColumnType::Bool,
            Some("0"),
            Some("0"),
            "",
            Some(DefaultValue::Bool(false)),
        ),
        (
            ColumnType::Int {
                size: IntSize::Normal,
                unsigned: false,
            },
            Some("-1"),
            Some("-1"),
            "",
            Some(DefaultValue::Int(-1)),
        ),
        (
            ColumnType::DateTime,
            Some("current_timestamp()"),
            Some("CURRENT_TIMESTAMP"),
            "DEFAULT_GENERATED",
            Some(DefaultValue::Now),
        ),
        (
            ColumnType::DateTime,
            Some("'2020-01-01 00:00:00'"),
            Some("2020-01-01 00:00:00"),
            "",
            s("2020-01-01 00:00:00"),
        ),
    ];

    for (ty, mariadb, mysql, extra, expected) in cases {
        assert_eq!(
            column_default(mariadb, "", &ty, true),
            expected,
            "MariaDB {mariadb:?}"
        );
        assert_eq!(
            column_default(mysql, &extra.to_ascii_lowercase(), &ty, false),
            expected,
            "MySQL {mysql:?}"
        );
    }
}
