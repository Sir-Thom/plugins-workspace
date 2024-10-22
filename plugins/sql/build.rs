// Copyright 2019-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

const COMMANDS: &[&str] = &["load", "execute", "select", "close"];

use std::{borrow::Cow, env::var, fs, io::{self}, path::{Path, PathBuf}, time::SystemTime};
use sqlx::migrate::MigrationType;
#[derive(Debug)]
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

#[derive(Debug)]
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

    let migrations_dir = var("MIGRATIONS_DIR").unwrap_or_default();
    let project_dir = var("PROJECT_DIR").unwrap_or_default();

    if migrations_dir.is_empty() || project_dir.is_empty() {
        eprintln!("MIGRATIONS_DIR and PROJECT_DIR must be set.");
        return; 
    }

    let src_dir = PathBuf::from(&project_dir).join("src-tauri/src");
    let migrations_rs_path = src_dir.join("migrations.rs");

    println!("Generated migrations file path: {:?}", migrations_rs_path);

    if needs_generation(Path::new(&migrations_dir), &migrations_rs_path) {
        match generate_migrations_from_directory(&migrations_dir) {
            Ok(current_migrations) => {
                if let Err(e) = write_migrations_rs(&migrations_rs_path, &current_migrations) {
                    eprintln!("Failed to write migrations.rs: {:?}", e);
                } else {
                    println!("Successfully generated migrations.rs");
                }
            }
            Err(e) => eprintln!("Failed to read migration files: {:?}", e),
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

fn generate_migrations_from_directory(directory: &str) -> Result<Vec<Migration>, io::Error> {
   let migrations =  fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().map_or(false, |ext| ext == "sql"))
        .map(|entry| {
            let path = entry.path();
            let filename = path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid filename"))?;
            let filename = filename.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid filename string"))?;
            let parts: Vec<&str> = filename.splitn(2, '-').collect();

            if parts.len() != 2 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid filename format"));
            }

            let version_str = parts[0];
            let description = parts[1].trim_end_matches(".sql").to_string();
            let sql = fs::read_to_string(&path).or_else(|_| Err(io::Error::new(io::ErrorKind::NotFound, "SQL file not found")))?;
            let version: i64 = version_str.parse().map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid version format"))?;

            Ok(Migration {
                version,
                description: Cow::Owned(description),
                sql: Cow::Owned(sql),
                kind: MigrationKind::Up,
            })
        })
        .collect::<Result<Vec<Migration>, io::Error>>()?;
        Ok(migrations)
}

fn write_migrations_rs(path: &Path, migrations: &[Migration]) -> io::Result<()> {
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
    vec![
"#, formatted_date);

    for migration in migrations {
        let sql_escaped = migration.sql.replace('"', r#"\""#);
        content.push_str(&format!(
            r#"        Migration {{
            version: {},
            description: "{}",
            sql: "{}",
            kind: MigrationKind::Up,
        }},
"#,
            migration.version, migration.description, sql_escaped
        ));
    }

    content.push_str("    ]\n}\n");
    fs::write(path, content)
}
