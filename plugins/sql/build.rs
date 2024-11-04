// Copyright 2019-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

const COMMANDS: &[&str] = &["load", "execute", "select", "close"];

use std::{borrow::Cow, env, fs, io::{self}, path::{Path, PathBuf}, time::SystemTime};
use std::collections::BTreeMap;
use sqlx::migrate::MigrationType;

#[derive(Debug, Clone)]
pub enum MigrationKind {
    Up,
    Down,
}

impl From<MigrationKind> for MigrationType {
    fn from(kind: MigrationKind) -> Self {
        match kind {
            MigrationKind::Up => Self::ReversibleUp,
            MigrationKind::Down => Self::ReversibleDown,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Migration {
    pub version: i64,
    pub description: Cow<'static, str>,
    pub sql: Cow<'static, str>,
    pub kind: MigrationKind,
}

fn parse_migration_line(line: &str) -> io::Result<Option<Migration>> {
    if !line.contains("Migration {") {
        return Ok(None);
    }

    let version = line.split("version:")
        .nth(1)
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse::<i64>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid version"))?;

    let description = extract_field(line, "description:")
        .unwrap_or_default()
        .to_string();

    let sql = extract_field(line, "sql:")
        .unwrap_or_default()
        .to_string();

    let kind = match extract_field(line, "kind:").unwrap_or("") {
        "Up" => MigrationKind::Up,
        "Down" => MigrationKind::Down,
        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "Unknown migration kind")),
    };

    Ok(Some(Migration {
        version,
        description: Cow::Owned(description.to_string()),
        sql: Cow::Owned(sql.to_string()),
        kind,
    }))
}

fn extract_field<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    line.split(field)
        .nth(1)?
        .split(',')
        .next()
        .map(|s| s.trim().trim_matches('"'))
}

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .global_api_script_path("./api-iife.js")
        .build();

    println!("Running build.rs");

    // Paths and environment setup
    let migrations_dir = env::var("MIGRATIONS_DIR").expect("MIGRATIONS_DIR not set");
    let project_dir = env::var("PROJECT_DIR").expect("PROJECT_DIR not set");
    let migrations_rs_path = Path::new(&project_dir).join("src-tauri/src/migrations.rs");
    println!("migrations.rs path: {:?}", migrations_rs_path);

    // Check if migrations generation is needed
    if needs_generation((&migrations_dir).as_ref(), &migrations_rs_path) {
        match generate_migrations_from_directory(&migrations_dir) {
            Ok(current_migrations) => {
                if let Err(e) = write_migrations(&migrations_rs_path, current_migrations) {
                    eprintln!("Error writing migrations.rs: {:?}", e);
                } else {
                    println!("Successfully wrote migrations.rs");
                }
            }
            Err(e) => eprintln!("Error generating migrations: {:?}", e),
        }
    }
}

fn needs_generation(migrations_dir: &Path, migrations_rs_path: &Path) -> bool {
    if !migrations_rs_path.exists() {
        return true;
    }

    let migrations_rs_modified = fs::metadata(migrations_rs_path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let sql_files: Vec<_> = match fs::read_dir(migrations_dir) {
        Ok(entries) => entries.filter_map(Result::ok)
            .filter(|entry| entry.path().extension().map_or(false, |ext| ext == "sql"))
            .collect(),
        Err(_) => {
            eprintln!("Failed to read migrations directory");
            return true;
        }
    };

    if sql_files.is_empty() {
        return true;
    }

    let any_newer = sql_files.iter().any(|entry| {
        entry.metadata()
            .and_then(|m| m.modified())
            .map(|time| time > migrations_rs_modified)
            .unwrap_or(false)
    });

    let migrations_count = count_migrations_in_file(migrations_rs_path);

    sql_files.len() != migrations_count || any_newer
}

fn count_migrations_in_file(path: &Path) -> usize {
    if let Ok(file_content) = fs::read_to_string(path) {
        file_content.lines()
            .filter(|line| line.contains("Migration {"))
            .count()
    } else {
        0
    }
}

// O(n*(k+1))
fn generate_migrations_from_directory(directory: &str) -> Result<BTreeMap<i64, Migration>, io::Error> {
    let mut migrations = BTreeMap::new();
    let entries = fs::read_dir(directory)?;

    for entry in entries.filter_map(Result::ok) {
        if entry.path().extension().map_or(false, |ext| ext == "sql") {
            let path = entry.path();
            let filename = path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid filename"))?.to_owned();
            let filename = filename.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid filename string"))?;
            let parts: Vec<&str> = filename.splitn(3, '-').collect();

            if parts.len() != 3 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid filename format"));
            }

            let version_str = parts[0];
            let description = parts[1];
            let kind_str = parts[2].trim_end_matches(".sql");

            let sql = fs::read_to_string(&path).or_else(|_| Err(io::Error::new(io::ErrorKind::NotFound, "SQL file not found")))?;
            let version: i64 = version_str.parse().map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid version format"))?;

            let kind = match kind_str {
                "up" => MigrationKind::Up,
                "down" => MigrationKind::Down,
                _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "Unknown migration kind")),
            };

            migrations.insert(version, Migration {
                version,
                description: Cow::Owned(description.to_string()),
                sql: Cow::Owned(sql.to_string()),
                kind,
            });
        }
    }

    Ok(migrations)
}

fn read_existing_migrations(path: &Path) -> io::Result<BTreeMap<i64, Migration>> {
    let mut migrations = BTreeMap::new();

    // Use 'String' to hold content and then convert to &'static str
    if let Ok(content) = fs::read_to_string(path) {
        for line in content.lines() {
            if let Ok(Some(migration)) = parse_migration_line(line) {
                // Make sure that description and sql are converted to &'static str.
                let description: &'static str = Box::leak(migration.description.to_string().into_boxed_str());
                let sql: &'static str = Box::leak(migration.sql.to_string().into_boxed_str());

                migrations.insert(migration.version, Migration {
                    version: migration.version,
                    description: Cow::Owned(description.to_string()),
                    sql: Cow::Owned(sql.to_string()),
                    kind: migration.kind,
                });
            }
        }
    }

    Ok(migrations)
}

pub fn write_migrations(migrations_rs_path: &PathBuf, new_migrations: BTreeMap<i64, Migration>) -> io::Result<()> {
    if let Some(parent_dir) = migrations_rs_path.parent() {
        fs::create_dir_all(parent_dir)?;
    }

    let mut migrations = read_existing_migrations(migrations_rs_path)?;

    // Insert new migrations directly into the sorted BTreeMap
    for migration in new_migrations.values() {
        migrations.insert(migration.version, migration.clone());
    }

    let content = generate_migrations_file_content(&migrations)?;
    fs::write(migrations_rs_path, content)?;

    println!("Successfully wrote {} migrations", migrations.len());
    Ok(())
}
fn generate_migrations_file_content(migrations: &BTreeMap<i64, Migration>) -> io::Result<String> {
    let current_time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs();

    let formatted_date = format!("{:?}", current_time);
    let mut content = format!(r#"/*
 * ===========================================================
 * WARNING: This is an auto-generated file.
 *
 * DO NOT MODIFY THIS FILE MANUALLY.
 *
 * Any changes made to this file will be overwritten
 * the next time it is generated.
 *
 * Generated on: {}
 * ===========================================================
 */
use tauri_plugin_sql::{{Migration, MigrationKind}};

pub fn migrations() -> Vec<Migration> {{
    vec!["#, formatted_date);

    for migration in migrations.values() {
        let sql_escaped = migration.sql.replace('"', r#"\""#);
        content.push_str(&format!(
            r#"
        Migration {{
            version: {},
            description: "{}",
            sql: "{}",
            kind: MigrationKind::{:?},
        }},"#,
            migration.version, migration.description, sql_escaped, migration.kind
        ));
    }
    content.push_str("\n    ]\n}\n");
    Ok(content)
}
