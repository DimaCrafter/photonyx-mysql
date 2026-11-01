use crate::dialect::MySqlDialect;
use json::{object, JsonValue};
use photonyx_models::{check::check, ir::*, parse::parse_entity};
use photonyx_sql_shared::{
    compare::drift, dialect::SqlSession, import::import, migrate, migration, plan, schema::*,
    to_db, SqlDialect,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut result = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            if let Ok(value) = u8::from_str_radix(&raw[i + 1..i + 3], 16) {
                result.push(value);
                i += 3;
                continue;
            }
        }

        result.push(bytes[i]);
        i += 1;
    }

    return String::from_utf8_lossy(&result).into_owned();
}

/// `mysql://user:password@host:port/database` --> `db.<name>` config branch
fn config_of(url: &str) -> JsonValue {
    let rest = url.strip_prefix("mysql://").expect("mysql:// URL");
    let (auth, rest) = rest.rsplit_once('@').expect("user@host in the URL");
    let (user, password) = auth.split_once(':').unwrap_or((auth, ""));
    let (address, database) = rest.split_once('/').expect("/database in the URL");
    let (host, port) = address.split_once(':').unwrap_or((address, "3306"));

    return object! {
        "driver": "mysql",
        "host": host,
        "port": port.parse::<u16>().expect("port"),
        "name": percent_decode(database),
        "username": percent_decode(user),
        "password": percent_decode(password)
    };
}

/// Configured servers: `(label, config)`
fn servers() -> Vec<(&'static str, JsonValue)> {
    let mut result = Vec::new();
    for (label, key) in [
        ("mysql", "PHOTONYX_TEST_MYSQL_URL"),
        ("mariadb", "PHOTONYX_TEST_MARIADB_URL"),
    ] {
        if let Ok(url) = env::var(key) {
            if !url.is_empty() {
                result.push((label, config_of(&url)));
            }
        }
    }

    if result.is_empty() {
        eprintln!("PHOTONYX_TEST_MYSQL_URL / PHOTONYX_TEST_MARIADB_URL are not set, skipped");
    }

    return result;
}

fn connect(config: &JsonValue) -> Box<dyn SqlSession> {
    return MySqlDialect.connect(config).expect("connect");
}

fn models(files: &[(&str, &str)]) -> ModelSet {
    let mut errors = Vec::new();
    let mut entities = Vec::new();
    for (name, source) in files {
        entities.push(
            parse_entity(
                &Path::new("main").join(format!("{name}.yml")),
                source,
                &mut errors,
            )
            .expect("entity"),
        );
    }

    entities.sort_by(|a, b| a.name.cmp(&b.name));
    let set = ModelSet {
        groups: vec![Group {
            name: "main".to_owned(),
            dir: "main".into(),
            entities,
        }],
    };
    check(&set, &mut errors);
    assert!(errors.is_empty(), "model errors: {errors:#?}");
    return set;
}

/// Drops the test tables of `prefix` (FKs first, so the order does not matter)
fn drop_tables(session: &mut dyn SqlSession, prefix: &str) {
    let schema = MySqlDialect.introspect(session).expect("introspect");
    for table in schema
        .tables
        .values()
        .filter(|table| table.name.starts_with(prefix))
    {
        for fk in &table.foreign_keys {
            session
                .exec(&format!(
                    "ALTER TABLE `{}` DROP FOREIGN KEY `{}`",
                    table.name, fk.name
                ))
                .expect("drop fk");
        }
    }

    for table in schema.tables.keys().filter(|name| name.starts_with(prefix)) {
        session
            .exec(&format!("DROP TABLE `{table}`"))
            .expect("drop table");
    }
}

/// Runs `plan(from --> to)` statements on the session
fn migrate_to(
    session: &mut dyn SqlSession,
    from: &ModelSet,
    to: &ModelSet,
    defaults: &[DialectBlock],
) -> usize {
    let result = plan(from, to, "main", &MySqlDialect, defaults).expect("plan");
    let statements = MySqlDialect.render(&result.ops);
    for statement in &statements {
        session
            .exec(&statement.sql)
            .unwrap_or_else(|error| panic!("{}\n{error}", statement.sql));
    }

    return statements.len();
}

fn assert_no_drift(
    session: &mut dyn SqlSession,
    set: &ModelSet,
    defaults: &[DialectBlock],
    prefix: &str,
    label: &str,
) -> DbSchema {
    let mut actual = MySqlDialect.introspect(session).expect("introspect");
    actual.tables.retain(|name, _| name.starts_with(prefix));
    let expected = to_db(set, "main", &MySqlDialect, defaults).expect("to_db");
    let result = drift(&actual, &expected);
    assert!(
        result.is_empty(),
        "{label}: drift {:#?}",
        photonyx_sql_shared::compare::describe(&result)
    );
    return actual;
}

const KIND: &str = "kind: enum\nvalues:\n  a:\n  bc: { db: \"b'c\" }\n  top:\n";

const ALL: &str = r#"
kind: table
table: pnx_t_all
fields:
  id:     { type: u64, auto: true }
  u8:     u8
  i8:     { type: i8, default: -1 }
  u16:    u16
  i32:    ?i32
  i64:    i64
  f32:    { type: f32, default: 2 }
  f64:    { type: ?f64, default: 0.5 }
  flag:   { type: ?bool, default: false }
  active: { type: bool, default: true }
  name:   { type: string(64), default: "it's \\ ok", mysql: { collation: utf8mb4_bin } }
  code:   char(3)
  empty:  { type: string(8), default: "" }
  nul:    { type: ?string(8), default: "NULL" }
  body:   { type: text, default: "x'y" }
  note:   ?text
  at:     { type: datetime, default: now }
  fixed:  { type: datetime, default: "2020-01-01 00:00:00" }
  data:   ?json
  kind:   { type: enum, enum: PnxKind, default: top }
  status: { type: string(16), enum: PnxKind, default: a }
indexes:
  primary: [id]
  name:    { columns: [name, code], unique: true }
  at:      [at]
"#;

const REF: &str = r#"
kind: table
table: pnx_t_ref
fields:
  id:     { type: u32, auto: true }
  allId:  u64
  name:   { type: ?string(64), mysql: { collation: utf8mb4_bin } }
  code:   ?char(3)
  parent: ?u32
indexes:
  primary:          [id]
  allId:            { columns: [allId], link: PnxAll, delete: cascade, constraint: pnx_t_ref_all }
  pnx_t_ref_name:   { columns: [name, code], link: PnxAll.name, delete: set_null }
  pnx_t_ref_parent: { columns: [parent], link: PnxRef, delete: restrict }
"#;

#[test]
fn normalized_schema_is_the_same_on_both_servers() {
    let set = models(&[("PnxKind", KIND), ("PnxAll", ALL), ("PnxRef", REF)]);
    let defaults = [DialectBlock {
        dialect: "mysql".to_owned(),
        value: Raw::Map(vec![(
            "collation".to_owned(),
            Raw::Str("utf8mb4_general_ci".to_owned()),
        )]),
    }];

    let mut schemas = Vec::new();
    for (label, config) in servers() {
        let mut session = connect(&config);
        drop_tables(session.as_mut(), "pnx_t_");

        migrate_to(
            session.as_mut(),
            &ModelSet { groups: vec![Group::new_detached("main".to_owned())] },
            &set,
            &defaults,
        );
        let actual = assert_no_drift(session.as_mut(), &set, &defaults, "pnx_t_", label);

        // spot checks of the normalization (concept §3)
        let all = &actual.tables["pnx_t_all"];
        let column = |name: &str| all.find_column(name).unwrap().clone();
        assert_eq!(
            column("id").db_type,
            ColumnType::Int {
                size: IntSize::Big,
                unsigned: true
            },
            "{label}"
        );
        assert!(column("id").auto_increment);
        assert_eq!(column("flag").db_type, ColumnType::Bool);
        assert_eq!(column("flag").default, Some(DefaultValue::Bool(false)));
        assert_eq!(
            column("name").default,
            Some(DefaultValue::Str("it's \\ ok".to_owned()))
        );
        assert_eq!(column("name").collation.as_deref(), Some("utf8mb4_bin"));
        assert_eq!(column("code").collation, None);
        assert_eq!(
            column("empty").default,
            Some(DefaultValue::Str(String::new()))
        );
        assert_eq!(
            column("nul").default,
            Some(DefaultValue::Str("NULL".to_owned()))
        );
        assert_eq!(column("note").default, None);
        assert_eq!(
            column("body").default,
            Some(DefaultValue::Str("x'y".to_owned()))
        );
        assert_eq!(column("at").default, Some(DefaultValue::Now));
        assert_eq!(column("data").db_type, ColumnType::Json, "{label}");
        assert_eq!(column("data").collation, None, "{label}");
        assert_eq!(
            column("kind").db_type,
            ColumnType::Enum(vec!["a".to_owned(), "b'c".to_owned(), "top".to_owned()])
        );
        assert_eq!(
            all.options.get("collation").map(String::as_str),
            Some("utf8mb4_general_ci")
        );

        let fks = &actual.tables["pnx_t_ref"].foreign_keys;
        let fk = |name: &str| fks.iter().find(|fk| fk.name == name).unwrap().clone();
        assert_eq!(
            (
                fk("pnx_t_ref_all").index.as_str(),
                fk("pnx_t_ref_all").on_delete
            ),
            ("allId", OnDelete::Cascade)
        );
        assert_eq!(fk("pnx_t_ref_name").ref_index.as_deref(), Some("name"));
        assert_eq!(fk("pnx_t_ref_name").on_delete, OnDelete::SetNull);
        assert_eq!(fk("pnx_t_ref_parent").ref_index, None);

        // A table written by hand in the old style: display widths, FK without ON DELETE,
        // JSON, defaults of every kind
        session
            .exec(
                "CREATE TABLE pnx_t_legacy (
			id int(11) NOT NULL AUTO_INCREMENT,
			ref int(10) unsigned DEFAULT NULL,
			flag tinyint(1) NOT NULL DEFAULT '0',
			label varchar(32) NOT NULL DEFAULT 'other',
			meta json DEFAULT NULL,
			created datetime NOT NULL DEFAULT CURRENT_TIMESTAMP,
			PRIMARY KEY (id),
			KEY ref (ref),
			CONSTRAINT pnx_t_legacy_ref FOREIGN KEY (ref) REFERENCES pnx_t_ref (id)
		) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci",
            )
            .expect("legacy table");

        let mut actual = MySqlDialect
            .introspect(session.as_mut())
            .expect("introspect");
        actual.tables.retain(|name, _| name.starts_with("pnx_t_"));
        for table in actual.tables.values_mut() {
            // the engine and the charset are the same, but MySQL may report other row formats etc.
            table
                .options
                .retain(|key, _| key == "collation" || key == "charset" || key == "engine");
        }

        drop_tables(session.as_mut(), "pnx_t_");
        schemas.push((label, actual));
    }

    if let [(first_label, first), (second_label, second)] = schemas.as_slice() {
        assert_eq!(
            first, second,
            "{first_label} and {second_label} give different schemas"
        );
    }
}

#[test]
fn lock_is_per_database_and_session() {
    for (label, config) in servers() {
        let mut first = connect(&config);
        let mut second = connect(&config);
        MySqlDialect.lock(first.as_mut()).expect("lock");

        let rows = second
            .query(
                "SELECT IS_USED_LOCK(CONCAT('photonyx_migrations.', DATABASE())) IS NOT NULL",
                &[],
            )
            .expect("query");
        assert_eq!(rows[0][0].text().as_deref(), Some("1"), "{label}");
        let rows = second
            .query(
                "SELECT GET_LOCK(CONCAT('photonyx_migrations.', DATABASE()), 0)",
                &[],
            )
            .expect("query");
        assert_eq!(rows[0][0].text().as_deref(), Some("0"), "{label}");

        drop(first);
        MySqlDialect
            .lock(second.as_mut())
            .expect("lock after the first session is closed");
    }
}

/// Moves the service tables of a database that is already under migrations aside (`pnx_aside_*`),
/// so that the apply test can run there (MariaDB: only `rifty_billing2` may be used). Puts them
/// back on drop, also when the test fails. Uses its own connection.
struct ServiceTablesAside {
    session: Box<dyn SqlSession>,
    moved: Vec<&'static str>,
}

impl ServiceTablesAside {
    const PREFIX: &'static str = "pnx_aside_";

    fn new(config: &JsonValue, label: &str) -> Self {
        let mut session = connect(config);
        let mut moved = Vec::new();
        for table in [migrate::MIGRATIONS_TABLE, migrate::MODELS_TABLE] {
            let aside = format!("{}{table}", Self::PREFIX);
            assert!(
				!MySqlDialect.table_exists(session.as_mut(), &aside).unwrap(),
				"{label}: `{aside}` exists: a previous run was interrupted; rename it back to `{table}` by hand"
			);

            if MySqlDialect.table_exists(session.as_mut(), table).unwrap() {
                session
                    .exec(&format!("RENAME TABLE `{table}` TO `{aside}`"))
                    .expect("move aside");
                moved.push(table);
            }
        }

        return ServiceTablesAside { session, moved };
    }
}

impl Drop for ServiceTablesAside {
    fn drop(&mut self) {
        for table in [migrate::MIGRATIONS_TABLE, migrate::MODELS_TABLE] {
            // the ones created by the test
            let _ = self
                .session
                .exec(&format!("DROP TABLE IF EXISTS `{table}`"));
        }

        for table in &self.moved {
            if let Err(error) = self.session.exec(&format!(
                "RENAME TABLE `{}{table}` TO `{table}`",
                Self::PREFIX
            )) {
                eprintln!("failed to put `{table}` back: {error}");
            }
        }
    }
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!(
        "photonyx-mysql-apply-{label}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    return dir;
}

const FLOW_USER: &str = "kind: table\ntable: pnx_t_users\nfields:\n  id: { type: u32, auto: true }\n  email: ?string(64)\n  group: { type: string(32), enum: PnxGroup, default: user }\nindexes:\n  primary: [id]\n  email: { columns: [email], unique: true }\n";
const FLOW_ITEM: &str = "kind: table\ntable: pnx_t_items\nfields:\n  id: { type: u32, auto: true }\n  userId: u32\nindexes:\n  primary: [id]\n  userId: { columns: [userId], link: PnxUser, delete: cascade, constraint: pnx_t_items_user }\n";
const FLOW_GROUP: &str = "kind: enum\nvalues: [user, admin]\n";

/// Baseline file on an empty database --> `apply` --> drift is empty --> a model change --> `plan` -->
/// `apply` --> drift is empty; the start-up check sees old and new hashes
#[test]
fn apply_then_drift_is_empty() {
    for (label, config) in servers() {
        let _aside = ServiceTablesAside::new(&config, label);
        let mut session = connect(&config);
        drop_tables(session.as_mut(), "pnx_t_");
        let dir = temp_dir(label);
        let v1 = models(&[
            ("PnxGroup", FLOW_GROUP),
            ("PnxUser", FLOW_USER),
            ("PnxItem", FLOW_ITEM),
        ]);

        // baseline file of v1
        let ops = migrate::baseline_ops(&v1, "main", &MySqlDialect, &[]).unwrap();
        let text = migration::compose(
            &["baseline".to_owned()],
            &MySqlDialect.render(&ops),
            ops.len(),
        )
        .text;
        fs::write(dir.join(migrate::BASELINE_FILE), text).unwrap();

        let report = migrate::apply(
            session.as_mut(),
            &MySqlDialect,
            &dir,
            Some((&v1, "main")),
            &[],
        )
        .expect("apply");
        assert!(report.failure.is_none(), "{label}: {:?}", report.failure);
        assert_eq!(report.applied.len(), 1);
        assert!(
            migrate::drift_db(session.as_mut(), &MySqlDialect, &v1, "main", &[])
                .unwrap()
                .ops
                .iter()
                .all(|op| !op.get_table().starts_with("pnx_t_"))
        );
        assert_no_drift(session.as_mut(), &v1, &[], "pnx_t_", label);

        let old_hashes = migrate::model_hashes(&v1, "main");
        assert!(
            migrate::check_models(session.as_mut(), &MySqlDialect, &old_hashes)
                .unwrap()
                .is_ok()
        );

        // v2: a new field and a new value of the enum
        let v2 = models(&[
            ("PnxGroup", "kind: enum\nvalues: [user, admin, support]\n"),
            (
                "PnxUser",
                &FLOW_USER.replace(
                    "  group:",
                    "  nick: { type: string(16), default: \"\" }\n  group:",
                ),
            ),
            ("PnxItem", FLOW_ITEM),
        ]);
        let result = plan(&v1, &v2, "main", &MySqlDialect, &[]).unwrap();
        let text = migration::compose(
            &["add nick".to_owned()],
            &MySqlDialect.render(&result.ops),
            result.ops.len(),
        )
        .text;
        fs::write(dir.join("20261024-001-pnx-users-add-nick.sql"), text).unwrap();

        let report = migrate::apply(
            session.as_mut(),
            &MySqlDialect,
            &dir,
            Some((&v2, "main")),
            &[],
        )
        .expect("apply");
        assert_eq!(
            report
                .applied
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["20261024-001-pnx-users-add-nick.sql"]
        );
        assert_no_drift(session.as_mut(), &v2, &[], "pnx_t_", label);

        let check = migrate::check_models(session.as_mut(), &MySqlDialect, &old_hashes).unwrap();
        assert_eq!(check.mismatched, ["PnxUser"], "{label}");
        assert!(migrate::check_models(
            session.as_mut(),
            &MySqlDialect,
            &migrate::model_hashes(&v2, "main")
        )
        .unwrap()
        .is_ok());

        let state = migrate::status(session.as_mut(), &MySqlDialect, &dir).unwrap();
        assert!(state.pending.is_empty() && state.changed_files.is_empty() && state.missed.is_empty());

        drop_tables(session.as_mut(), "pnx_t_");
        let _ = fs::remove_dir_all(&dir);
    }
}

const STAMPED: &str = r#"
kind: table
table: pnx_t_stamped
fields:
  id:      { type: u32, auto: true }
  price:   u32
  qty:     u32
  total:   { type: ?u64, mysql: { generated: "price * qty" } }
  total_s: { type: ?u64, mysql: { generated: "(price * qty) + 1", stored: true } }
  code:    { type: string(36), mysql: { default: "uuid()" } }
  # MySQL: a generated column refers to a column with an expression default only after it
  label:   { type: ?string(40), mysql: { generated: "concat('#', upper(code))" } }
  created: { type: datetime, default: now }
  updated: { type: datetime, default: now, update: now }
  touched: { type: ?datetime, update: now }
indexes:
  primary: [id]
"#;

/// `ON UPDATE CURRENT_TIMESTAMP`, generated columns and expression defaults: created from the
/// models, introspected without drift on both servers; import gives models without drift too
#[test]
fn update_now_generated_and_expression_defaults_have_no_drift() {
    let set = models(&[("PnxStamped", STAMPED)]);

    for (label, config) in servers() {
        let mut session = connect(&config);
        drop_tables(session.as_mut(), "pnx_t_");

		let empty_model_set = ModelSet {
			groups: vec![Group::new_detached("main".to_owned())]
		};

        migrate_to(session.as_mut(), &empty_model_set, &set, &[]);
        let actual = assert_no_drift(session.as_mut(), &set, &[], "pnx_t_", label);

        let table = &actual.tables["pnx_t_stamped"];
        let column = |name: &str| table.find_column(name).unwrap().clone();
        assert!(
            column("updated").on_update_now
                && column("touched").on_update_now
                && !column("created").on_update_now,
            "{label}"
        );
        assert_eq!(
            column("updated").default,
            Some(DefaultValue::Now),
            "{label}"
        );
        assert_eq!(
            column("total").generated.map(|generated| generated.stored),
            Some(false),
            "{label}"
        );
        assert_eq!(
            column("total_s")
                .generated
                .map(|generated| generated.stored),
            Some(true),
            "{label}"
        );
        assert_eq!(column("total").default, None, "{label}");
        assert!(
            matches!(column("code").default, Some(DefaultValue::Expr(_))),
            "{label}: {:?}",
            column("code").default
        );

        // the database computes and sets them
        session
            .exec("INSERT INTO pnx_t_stamped (price, qty) VALUES (3, 4)")
            .expect("insert");
        let rows = session
            .query(
                "SELECT total, total_s, label, CHAR_LENGTH(code) FROM pnx_t_stamped",
                &[],
            )
            .expect("select");
        let values: Vec<String> = rows[0]
            .iter()
            .map(|value| value.text().unwrap_or_default())
            .collect();
        assert_eq!(values[..2], ["12".to_owned(), "13".to_owned()], "{label}");
        assert!(
            values[2].starts_with('#') && values[3] == "36",
            "{label}: {values:?}"
        );

        // import: the expressions go to the `mysql:` block, `update: now` to the field
        let draft = import(&actual, "main", Path::new("main"), &MySqlDialect, &[]).expect("import");
        let imported = ModelSet {
            groups: vec![draft.group],
        };
        let mut errors = Vec::new();
        check(&imported, &mut errors);
        assert!(errors.is_empty(), "{label}: {errors:#?}");
        assert_no_drift(session.as_mut(), &imported, &[], "pnx_t_", label);

        drop_tables(session.as_mut(), "pnx_t_");
    }
}
