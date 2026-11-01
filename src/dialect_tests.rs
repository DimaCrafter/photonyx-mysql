use crate::dialect::MySqlDialect;
use photonyx_models::{check::check, ir::*, parse::parse_entity};
use photonyx_sql_shared::{
    migrate::{split_statements, FileStatement}, plan, SqlDialect,
};
use std::path::Path;

fn models(files: &[(&str, &str)]) -> ModelSet {
    let mut errors = Vec::new();
    let mut entities = Vec::new();
    for (name, source) in files {
        let path = Path::new("main").join(format!("{name}.yml"));
        entities.push(parse_entity(&path, source, &mut errors).expect("entity"));
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

fn ddl_with(from: &ModelSet, to: &ModelSet, defaults: &[DialectBlock]) -> String {
    let result = plan(from, to, "main", &MySqlDialect, defaults).expect("plan");
    let statements = MySqlDialect.render(&result.ops);
    return statements
        .iter()
        .map(|statement| format!("{};\n", statement.sql))
        .collect();
}

fn ddl(from: &ModelSet, to: &ModelSet) -> String {
    return ddl_with(from, to, &[]);
}

fn empty() -> ModelSet {
    return ModelSet {
        groups: vec![Group::new_detached("main".to_owned())],
    };
}

const ALL_TYPES: &str = r#"
kind: table
table: all_types
fields:
  id:      { type: u64, auto: true }
  u8:      u8
  u16:     u16
  u32:     { type: u32, default: 7 }
  i8:      { type: i8, default: -1 }
  i16:     i16
  i32:     ?i32
  i64:     i64
  f32:     { type: f32, default: 2 }
  f64:     { type: ?f64, default: 0.5 }
  flag:    { type: bool, default: true }
  name:    { type: string(64), default: "it's \\ ok", mysql: { collation: utf8mb4_bin } }
  code:    char(3)
  body:    { type: text, default: "" }
  at:      { type: datetime, default: now }
  fixed:   { type: datetime, default: "2020-01-01 00:00:00" }
  data:    ?json
indexes:
  primary: [id]
  name:    { columns: [name, code], unique: true, mysql: { using: hash } }
  at:      [at]
mysql: { engine: InnoDB, collation: utf8mb4_unicode_ci }
"#;

#[test]
fn create_all_types() {
    let defaults = [DialectBlock {
        dialect: "mysql".to_owned(),
        value: Raw::Map(vec![("charset".to_owned(), Raw::Str("utf8mb4".to_owned()))]),
    }];
    assert_eq!(
        ddl_with(&empty(), &models(&[("AllTypes", ALL_TYPES)]), &defaults),
        r#"CREATE TABLE `all_types` (
  `id` bigint unsigned NOT NULL AUTO_INCREMENT,
  `u8` tinyint unsigned NOT NULL,
  `u16` smallint unsigned NOT NULL,
  `u32` int unsigned NOT NULL DEFAULT 7,
  `i8` tinyint NOT NULL DEFAULT -1,
  `i16` smallint NOT NULL,
  `i32` int NULL,
  `i64` bigint NOT NULL,
  `f32` float NOT NULL DEFAULT 2,
  `f64` double NULL DEFAULT 0.5,
  `flag` tinyint(1) NOT NULL DEFAULT 1,
  `name` varchar(64) COLLATE utf8mb4_bin NOT NULL DEFAULT 'it''s \\ ok',
  `code` char(3) NOT NULL,
  `body` text NOT NULL DEFAULT (''),
  `at` datetime NOT NULL DEFAULT CURRENT_TIMESTAMP,
  `fixed` datetime NOT NULL DEFAULT '2020-01-01 00:00:00',
  `data` json NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `name` (`name`, `code`) USING HASH,
  KEY `at` (`at`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
"#
    );
}

const ROLE: &str = "
kind: enum
values:
  user:    { int: 0 }
  support: { int: 1, db: help-desk }
  admin:   { int: 2 }
";

const ACCOUNT: &str = "
kind: table
table: accounts
fields:
  id:        { type: u32, auto: true }
  as_string: { type: string(16), enum: Role, default: support }
  as_int:    { type: ?u8, enum: Role, default: admin }
  as_native: { type: enum, enum: Role, default: user }
indexes:
  primary: [id]
";

#[test]
fn enum_storage_kinds() {
    let before = models(&[("Role", ROLE), ("Account", ACCOUNT)]);
    assert_eq!(
        ddl(&empty(), &before),
        "CREATE TABLE `accounts` (
  `id` int unsigned NOT NULL AUTO_INCREMENT,
  `as_string` varchar(16) NOT NULL DEFAULT 'help-desk',
  `as_int` tinyint unsigned NULL DEFAULT 2,
  `as_native` enum('user','help-desk','admin') NOT NULL DEFAULT 'user',
  PRIMARY KEY (`id`)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
"
    );

    // a new value: only the native enum changes
    let added = models(&[
        ("Role", &format!("{ROLE}  root:    {{ int: 3 }}\n")),
        ("Account", ACCOUNT),
    ]);
    assert_eq!(ddl(&before, &added), "ALTER TABLE `accounts` MODIFY COLUMN `as_native` enum('user','help-desk','admin','root') NOT NULL DEFAULT 'user';\n");
}

const TEAM: &str = "
kind: table
table: teams
fields:
  id:     { type: u32, auto: true }
  slug:   string(32)
  region: char(2)
  owner:  ?u32
indexes:
  primary: [id]
  slug_region: { columns: [slug, region], unique: true }
  owner: { columns: [owner], link: Member, delete: set_null, constraint: teams_owner_fk }
";

const MEMBER: &str = "
kind: table
table: members
fields:
  id:        { type: u32, auto: true }
  team_slug: string(32)
  region:    char(2)
  mentor:    ?u32
indexes:
  primary: [id]
  team: { columns: [team_slug, region], link: Team.slug_region, delete: cascade }
  mentor: { columns: [mentor], link: Member, delete: none }
";

#[test]
fn foreign_keys_cycle_and_unique_target() {
    assert_eq!(ddl(&empty(), &models(&[("Team", TEAM), ("Member", MEMBER)])), "CREATE TABLE `members` (
  `id` int unsigned NOT NULL AUTO_INCREMENT,
  `team_slug` varchar(32) NOT NULL,
  `region` char(2) NOT NULL,
  `mentor` int unsigned NULL,
  PRIMARY KEY (`id`),
  KEY `team` (`team_slug`, `region`),
  KEY `mentor` (`mentor`),
  CONSTRAINT `mentor` FOREIGN KEY (`mentor`) REFERENCES `members` (`id`) ON DELETE NO ACTION
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
CREATE TABLE `teams` (
  `id` int unsigned NOT NULL AUTO_INCREMENT,
  `slug` varchar(32) NOT NULL,
  `region` char(2) NOT NULL,
  `owner` int unsigned NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `slug_region` (`slug`, `region`),
  KEY `owner` (`owner`),
  CONSTRAINT `teams_owner_fk` FOREIGN KEY (`owner`) REFERENCES `members` (`id`) ON DELETE SET NULL
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
ALTER TABLE `members` ADD CONSTRAINT `team` FOREIGN KEY (`team_slug`, `region`) REFERENCES `teams` (`slug`, `region`) ON DELETE CASCADE;
");
}

#[test]
fn alter_table() {
    let before = models(&[("Team", TEAM), ("Member", MEMBER)]);
    let member = MEMBER
		.replace("  mentor:    ?u32\n", "  mentor:    ?u32\n  nick:      { type: string(16), default: anon }\n  bio:       ?text\n")
		.replace("  team_slug: string(32)\n", "  team_slug: string(32)\n  joined:    { type: datetime, default: now }\n")
		.replace("mentor: { columns: [mentor], link: Member, delete: none }", "mentor: { columns: [mentor], link: Member, delete: set_null }\n  nick: { columns: [nick], unique: true }");
    let team = TEAM
        .replace("  slug:   string(32)\n", "  slug:   string(48)\n")
        .replace("  team_slug: string(32)", "");
    let member = member.replace("team_slug: string(32)", "team_slug: string(48)");

    assert_eq!(ddl(&before, &models(&[("Team", &team), ("Member", &member)])), "\
ALTER TABLE `members` DROP FOREIGN KEY `team`;
ALTER TABLE `members` DROP FOREIGN KEY `mentor`;
ALTER TABLE `members` MODIFY COLUMN `team_slug` varchar(48) NOT NULL;
ALTER TABLE `members` ADD COLUMN `joined` datetime NOT NULL DEFAULT CURRENT_TIMESTAMP AFTER `team_slug`;
ALTER TABLE `members` ADD COLUMN `nick` varchar(16) NOT NULL DEFAULT 'anon' AFTER `mentor`;
ALTER TABLE `members` ADD COLUMN `bio` text NULL AFTER `nick`;
ALTER TABLE `teams` MODIFY COLUMN `slug` varchar(48) NOT NULL;
ALTER TABLE `members` ADD UNIQUE KEY `nick` (`nick`);
ALTER TABLE `members` ADD CONSTRAINT `team` FOREIGN KEY (`team_slug`, `region`) REFERENCES `teams` (`slug`, `region`) ON DELETE CASCADE;
ALTER TABLE `members` ADD CONSTRAINT `mentor` FOREIGN KEY (`mentor`) REFERENCES `members` (`id`) ON DELETE SET NULL;
");
}

#[test]
fn drop_column_index_and_tombstone() {
    let before = models(&[
        ("Role", ROLE),
        ("Account", ACCOUNT),
        ("Team", TEAM),
        ("Member", MEMBER),
    ]);
    let account = ACCOUNT.replace(
        "  as_int:    { type: ?u8, enum: Role, default: admin }\n",
        "",
    ) + "deleted: true\n";
    let team = TEAM.replace("  slug_region: { columns: [slug, region], unique: true }\n", "  slug_region: { columns: [slug, region], unique: true }\n  region: [region]\n")
		.replace("  owner:  ?u32\n", "")
		.replace("  owner: { columns: [owner], link: Member, delete: set_null, constraint: teams_owner_fk }\n", "")
		+ "mysql: { charset: utf8mb4, collation: utf8mb4_general_ci }\n";

    assert_eq!(
        ddl(
            &before,
            &models(&[
                ("Role", ROLE),
                ("Account", &account),
                ("Team", &team),
                ("Member", MEMBER)
            ])
        ),
        "\
ALTER TABLE `teams` DROP FOREIGN KEY `teams_owner_fk`;
ALTER TABLE `teams` DROP INDEX `owner`;
ALTER TABLE `teams` DEFAULT CHARSET=utf8mb4, COLLATE=utf8mb4_general_ci;
ALTER TABLE `teams` DROP COLUMN `owner`;
ALTER TABLE `teams` ADD KEY `region` (`region`);
DROP TABLE `accounts`;
"
    );
}

#[test]
fn primary_key_and_first_column() {
    let before = models(&[("Price", "kind: table\ntable: prices\nfields:\n  tariff: u32\n  period: u32\nindexes:\n  primary: [tariff]\n")]);
    let after = models(&[("Price", "kind: table\ntable: prices\nfields:\n  id: { type: u32, auto: true }\n  tariff: u32\n  period: u32\nindexes:\n  primary: [id]\n  tariff_period: { columns: [tariff, period], unique: true }\n")]);
    assert_eq!(
        ddl(&before, &after),
        "\
ALTER TABLE `prices` DROP PRIMARY KEY;
ALTER TABLE `prices` ADD COLUMN `id` int unsigned NOT NULL FIRST;
ALTER TABLE `prices` ADD PRIMARY KEY (`id`);
ALTER TABLE `prices` ADD UNIQUE KEY `tariff_period` (`tariff`, `period`);
ALTER TABLE `prices` MODIFY COLUMN `id` int unsigned NOT NULL AUTO_INCREMENT;
"
    );
}

#[test]
fn dialect_block_errors() {
    let set = models(&[(
        "Thing",
        "kind: table\ntable: things\nfields:\n  id: { type: u32, mysql: { charset: latin1 } }\n",
    )]);
    let error = plan(&empty(), &set, "main", &MySqlDialect, &[]).unwrap_err();
    assert!(error.contains("unknown key `charset`"), "{error}");

    let set = models(&[(
        "Thing",
        "kind: table\ntable: things\nfields:\n  id: string(20000)\n",
    )]);
    let error = plan(&empty(), &set, "main", &MySqlDialect, &[]).unwrap_err();
    assert!(error.contains("use text"), "{error}");
}

const STAMPED: &str = r#"
kind: table
table: stamped
fields:
  id:      { type: u32, auto: true }
  price:   u32
  qty:     u32
  total:   { type: ?u64, mysql: { generated: "price * qty" } }
  total_s: { type: ?u64, mysql: { generated: "(price * qty) + 1", stored: true } }
  code:    { type: string(36), mysql: { default: "(uuid())" } }
  created: { type: datetime, default: now }
  updated: { type: datetime, default: now, update: now }
  touched: { type: ?datetime, update: now }
indexes:
  primary: [id]
"#;

#[test]
fn update_now_generated_and_expression_defaults() {
    assert_eq!(
        ddl(&empty(), &models(&[("Stamped", STAMPED)])),
        "\
CREATE TABLE `stamped` (
  `id` int unsigned NOT NULL AUTO_INCREMENT,
  `price` int unsigned NOT NULL,
  `qty` int unsigned NOT NULL,
  `total` bigint unsigned GENERATED ALWAYS AS (price * qty) VIRTUAL,
  `total_s` bigint unsigned GENERATED ALWAYS AS ((price * qty) + 1) STORED,
  `code` varchar(36) NOT NULL DEFAULT (uuid()),
  `created` datetime NOT NULL DEFAULT CURRENT_TIMESTAMP,
  `updated` datetime NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
  `touched` datetime NULL ON UPDATE CURRENT_TIMESTAMP,
  PRIMARY KEY (`id`)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
"
    );

    // a changed expression is a MODIFY of the column
    let changed = STAMPED.replace(
        "generated: \"price * qty\" }",
        "generated: \"price * qty * 2\" }",
    );
    assert_eq!(ddl(&models(&[("Stamped", STAMPED)]), &models(&[("Stamped", &changed)])), "\
ALTER TABLE `stamped` MODIFY COLUMN `total` bigint unsigned GENERATED ALWAYS AS (price * qty * 2) VIRTUAL;
");
}

#[test]
fn generated_and_expression_default_errors() {
    let table = |field: &str| {
        format!("kind: table\ntable: things\nfields:\n  id: {{ type: u32, auto: true }}\n  {field}\nindexes:\n  primary: [id]\n")
    };
    let error_of = |field: &str| {
        plan(
            &empty(),
            &models(&[("Thing", &table(field))]),
            "main",
            &MySqlDialect,
            &[],
        )
        .unwrap_err()
    };

    let error = error_of("total: { type: u64, mysql: { generated: \"id * 2\" } }");
    assert!(error.contains("must be nullable"), "{error}");
    let error = error_of("total: { type: ?u64, mysql: { stored: true } }");
    assert!(
        error.contains("only together with mysql.generated"),
        "{error}"
    );
    let error =
        error_of("code: { type: string(36), default: \"x\", mysql: { default: \"uuid()\" } }");
    assert!(
        error.contains("`default` and `mysql.default` together"),
        "{error}"
    );
    let error = error_of("total: { type: ?u64, default: 1, mysql: { generated: \"id * 2\" } }");
    assert!(
        error.contains("a generated column can not have a default"),
        "{error}"
    );
    let error = error_of("code: { type: string(36), mysql: { default: 5 } }");
    assert!(
        error.contains("mysql.default: expected a non-empty string"),
        "{error}"
    );
}

#[test]
fn split_statements_by_semicolons() {
    let text = "-- header; with a semicolon\n--\n\nCREATE TABLE `a;b` (\n  x varchar(3) DEFAULT 'it''s; \\' ok'\n);\n/* block; comment */ ALTER TABLE a ADD y int; # trailing\n# line comment;\nINSERT INTO t VALUES (\"a;\"), ('--not a comment');\n\n-- end\n";
    assert_eq!(
        split_statements(text, &MySqlDialect),
        [
            FileStatement {
                sql: "CREATE TABLE `a;b` (\n  x varchar(3) DEFAULT 'it''s; \\' ok'\n)".to_owned(),
                line: 4
            },
            FileStatement {
                sql: "ALTER TABLE a ADD y int".to_owned(),
                line: 7
            },
            FileStatement {
                sql: "INSERT INTO t VALUES (\"a;\"), ('--not a comment')".to_owned(),
                line: 9
            }
        ]
    );

    // without a trailing semicolon
    assert_eq!(
        split_statements("SELECT 1;\nSELECT 2", &MySqlDialect),
        [
            FileStatement {
                sql: "SELECT 1".to_owned(),
                line: 1
            },
            FileStatement {
                sql: "SELECT 2".to_owned(),
                line: 2
            }
        ]
    );

    // `#` comments, backslash escapes, no dollar quotes
    assert_eq!(
        split_statements("SELECT 'a\\';b'; # c;\nSELECT $$;$$", &MySqlDialect),
        [
            FileStatement {
                sql: "SELECT 'a\\';b'".to_owned(),
                line: 1
            },
            FileStatement {
                sql: "SELECT $$".to_owned(),
                line: 2
            },
            FileStatement {
                sql: "$$".to_owned(),
                line: 2
            }
        ]
    );
}
