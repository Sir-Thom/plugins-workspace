// Copyright 2019-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

const COMMANDS: &[&str] = &["load", "execute", "select", "close"];

use std::{borrow::Cow, env, fs, io::{self}, path::{Path, PathBuf}, time::SystemTime};
use std::collections::HashMap;
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


fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .global_api_script_path("./api-iife.js")
        .build();

    println!("Running build.rs");

    let migrations_dir = env::var("MIGRATIONS_DIR").expect("MIGRATIONS_DIR not set");
    let project_dir = env::var("PROJECT_DIR").expect("PROJECT_DIR not set");

    println!("MIGRATIONS_DIR: {}", migrations_dir);
    println!("PROJECT_DIR: {}", project_dir);

    let migrations_rs_path = Path::new(&project_dir).join("src-tauri/src/migrations.rs");
    println!("migrations.rs path: {:?}", migrations_rs_path);

    // Check if generation is needed
    if needs_generation(Path::new(&migrations_dir), &migrations_rs_path) {
        println!("Generation of migrations.rs is needed.");

        match generate_migrations_from_directory(&migrations_dir) {
            Ok(current_migrations) => {
                if let Err(e) = write_migrations(&migrations_rs_path, Box::from(current_migrations)) {
                    eprintln!("Error writing migrations.rs: {:?}", e);
                } else {
                    println!("Successfully wrote migrations.rs");
                }
            }
            Err(e) => eprintln!("Error generating migrations: {:?}", e),
        }
    } else {
        println!("No need to regenerate migrations.rs");
    }

    println!("cargo:rerun-if-changed={}", migrations_dir);
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
//O(n*(k+1))
fn generate_migrations_from_directory(directory: &str) -> Result<Vec<Migration>, io::Error> {
    let migrations = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().map_or(false, |ext| ext == "sql"))
        .map(|entry| {
            let path = entry.path();
            let filename = path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid filename"))?;
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

            Ok(Migration {
                version,
                description: Cow::Owned(description.to_string()),
                sql: Cow::Owned(sql),
                kind,
            })
        })
        .collect::<Result<Vec<Migration>, io::Error>>()?;

    Ok(migrations)
}

fn read_existing_migrations(path: &Path) -> Result<HashMap<i64, Migration>, io::Error> {
    let mut migrations = HashMap::new();

    if let Ok(content) = fs::read_to_string(path) {
        for line in content.lines() {
            if line.contains("Migration {") {
                let version_str = line.split_whitespace().find(|s| s.starts_with("version:"))
                    .and_then(|s| s.split(':').nth(1))
                    .map(|s| s.trim().parse::<i64>());

                if let Some(Ok(version_num)) = version_str {
                    let description = line.split_whitespace().find(|s| s.starts_with("description:"))
                        .and_then(|s| s.split(':').nth(1))
                        .unwrap_or("").trim().to_string();

                    let sql = line.split_whitespace().find(|s| s.starts_with("sql:"))
                        .and_then(|s| s.split(':').nth(1))
                        .unwrap_or("").trim().to_string();

                    let kind_str = line.split_whitespace().find(|s| s.starts_with("kind:"))
                        .and_then(|s| s.split(':').nth(1))
                        .unwrap_or("").trim();

                    let kind = match kind_str {
                        "Up" => MigrationKind::Up,
                        "Down" => MigrationKind::Down,
                        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "Unknown migration kind")),
                    };

                    migrations.insert(version_num, Migration {
                        version: version_num,
                        description: Cow::Owned(description),
                        sql: Cow::Owned(sql),
                        kind,
                    });
                }
            }
        }
    }

    Ok(migrations)
}


fn write_migrations(migrations_rs_path: &PathBuf, new_migrations: Box<[Migration]>) -> std::io::Result<()> {
    if let Some(parent_dir) = migrations_rs_path.parent() {
        fs::create_dir_all(parent_dir)?;
    }

    let existing_versions = read_existing_migrations(migrations_rs_path)?;
    let existing_migrations = read_existing_migrations(migrations_rs_path)?;
    // Debug: Log migration counts
    println!("Existing migrations count: {}", existing_versions.len());
    println!("New migrations count: {}", new_migrations.len());

    let mut all_migrations = Vec::new();

    for existing_migration in existing_migrations.values() {
        all_migrations.push(existing_migration.clone());
    }

    for migration in &new_migrations {
        if !existing_migrations.contains_key(&migration.version) {
            all_migrations.push(migration.clone());
        }
    }

    all_migrations.sort_by_key(|m| m.version);

    println!("Total migrations to write: {}", all_migrations.len());


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

    for migration in &all_migrations {
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
    fs::write(migrations_rs_path, content)?;
    println!("Successfully wrote {} migrations to migrations.rs", all_migrations.len());

    Ok(())
}