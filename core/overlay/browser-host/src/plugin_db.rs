//! Per-plugin SQLite — Electron `plugin-db.ts` parity (`<pluginDir>/data/app.db`).

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value, json};

fn db_path(plugin_dir: &Path) -> std::path::PathBuf {
    plugin_dir.join("data").join("app.db")
}

fn ensure(plugin_dir: &Path) -> Result<Connection, String> {
    let data_dir = plugin_dir.join("data");
    std::fs::create_dir_all(&data_dir).map_err(|e| format!("db mkdir failed: {e}"))?;
    let conn = Connection::open(db_path(plugin_dir)).map_err(|e| format!("db open failed: {e}"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )
    .map_err(|e| format!("db _meta failed: {e}"))?;
    migrate_store_json(plugin_dir, &conn)?;
    Ok(conn)
}

/// One-shot: copy legacy `data/store.json` into `kv` (Electron parity).
fn migrate_store_json(plugin_dir: &Path, conn: &Connection) -> Result<(), String> {
    let flag: Option<String> = conn
        .query_row(
            "SELECT value FROM _meta WHERE key = 'store_json_migrated'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("db migrate flag read failed: {e}"))?;
    if flag.as_deref() == Some("1") {
        return Ok(());
    }

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS kv (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )
    .map_err(|e| format!("db kv create failed: {e}"))?;

    let store_path = plugin_dir.join("data").join("store.json");
    let mut migration_ok = true;
    if store_path.exists() {
        match std::fs::read_to_string(&store_path) {
            Ok(raw) => {
                let raw = raw.strip_prefix('\u{FEFF}').unwrap_or(raw.as_str());
                match serde_json::from_str::<Value>(raw) {
                    Ok(Value::Object(map)) => {
                        for (key, value) in map {
                            let encoded =
                                serde_json::to_string(&value).unwrap_or_else(|_| "null".into());
                            if conn
                                .execute(
                                    "INSERT INTO kv (key, value) VALUES (?1, ?2)
                                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                                    params![key, encoded],
                                )
                                .is_err()
                            {
                                migration_ok = false;
                                break;
                            }
                        }
                    }
                    _ => migration_ok = false,
                }
            }
            Err(_) => migration_ok = false,
        }
    }

    if !migration_ok {
        return Ok(());
    }

    conn.execute(
        "INSERT INTO _meta (key, value) VALUES ('store_json_migrated', '1')
         ON CONFLICT(key) DO UPDATE SET value = '1'",
        [],
    )
    .map_err(|e| format!("db migrate flag write failed: {e}"))?;
    Ok(())
}

pub fn exec(plugin_dir: &Path, sql: &str) -> Result<String, String> {
    let conn = ensure(plugin_dir)?;
    conn.execute_batch(sql)
        .map_err(|e| format!("db.exec failed: {e}"))?;
    Ok(String::new())
}

pub fn run(plugin_dir: &Path, sql: &str, params_json: &[Value]) -> Result<String, String> {
    let conn = ensure(plugin_dir)?;
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("db.run prepare failed: {e}"))?;
    bind_params(&mut stmt, params_json)?;
    stmt.raw_execute()
        .map_err(|e| format!("db.run failed: {e}"))?;
    Ok(json!({
        "changes": conn.changes(),
        "lastInsertRowid": conn.last_insert_rowid(),
    })
    .to_string())
}

pub fn get(plugin_dir: &Path, sql: &str, params_json: &[Value]) -> Result<String, String> {
    let conn = ensure(plugin_dir)?;
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("db.get prepare failed: {e}"))?;
    bind_params(&mut stmt, params_json)?;
    let mut rows = stmt.raw_query();
    match rows.next() {
        Ok(Some(row)) => Ok(row_to_json(row)?.to_string()),
        Ok(None) => Ok("null".into()),
        Err(e) => Err(format!("db.get failed: {e}")),
    }
}

pub fn all(plugin_dir: &Path, sql: &str, params_json: &[Value]) -> Result<String, String> {
    let conn = ensure(plugin_dir)?;
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("db.all prepare failed: {e}"))?;
    bind_params(&mut stmt, params_json)?;
    let mut rows = stmt.raw_query();
    let mut out = Vec::new();
    loop {
        match rows.next() {
            Ok(Some(row)) => out.push(row_to_json(row)?),
            Ok(None) => break,
            Err(e) => return Err(format!("db.all failed: {e}")),
        }
    }
    Ok(Value::Array(out).to_string())
}

fn bind_params(stmt: &mut rusqlite::Statement<'_>, params_json: &[Value]) -> Result<(), String> {
    for (i, v) in params_json.iter().enumerate() {
        let idx = i + 1;
        match v {
            Value::Null => stmt
                .raw_bind_parameter(idx, rusqlite::types::Null)
                .map_err(|e| format!("db bind failed: {e}"))?,
            Value::Bool(b) => stmt
                .raw_bind_parameter(idx, *b)
                .map_err(|e| format!("db bind failed: {e}"))?,
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    stmt.raw_bind_parameter(idx, i)
                        .map_err(|e| format!("db bind failed: {e}"))?;
                } else if let Some(f) = n.as_f64() {
                    stmt.raw_bind_parameter(idx, f)
                        .map_err(|e| format!("db bind failed: {e}"))?;
                } else {
                    return Err("db bind failed: unsupported number".into());
                }
            }
            Value::String(s) => stmt
                .raw_bind_parameter(idx, s.as_str())
                .map_err(|e| format!("db bind failed: {e}"))?,
            other => stmt
                .raw_bind_parameter(idx, other.to_string())
                .map_err(|e| format!("db bind failed: {e}"))?,
        }
    }
    Ok(())
}

fn row_to_json(row: &rusqlite::Row<'_>) -> Result<Value, String> {
    let stmt = row.as_ref();
    let mut map = Map::new();
    for i in 0..stmt.column_count() {
        let name = stmt
            .column_name(i)
            .map_err(|e| format!("db column name failed: {e}"))?
            .to_string();
        let val: rusqlite::types::Value = row
            .get(i)
            .map_err(|e| format!("db column read failed: {e}"))?;
        map.insert(name, sqlite_value_to_json(val));
    }
    Ok(Value::Object(map))
}

fn sqlite_value_to_json(v: rusqlite::types::Value) -> Value {
    match v {
        rusqlite::types::Value::Null => Value::Null,
        rusqlite::types::Value::Integer(i) => json!(i),
        rusqlite::types::Value::Real(f) => json!(f),
        rusqlite::types::Value::Text(s) => Value::String(s),
        rusqlite::types::Value::Blob(b) => Value::Array(b.into_iter().map(|x| json!(x)).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_plugin_dir() -> std::path::PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("go-db-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn exec_run_get_all_roundtrip() {
        let dir = temp_plugin_dir();
        exec(&dir, "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)").unwrap();
        let run = run(&dir, "INSERT INTO t (name) VALUES (?1)", &[json!("a")]).unwrap();
        let run_v: Value = serde_json::from_str(&run).unwrap();
        assert_eq!(run_v["changes"], 1);

        let row = get(&dir, "SELECT name FROM t WHERE id = ?1", &[json!(1)]).unwrap();
        let row_v: Value = serde_json::from_str(&row).unwrap();
        assert_eq!(row_v["name"], "a");

        let all_s = all(&dir, "SELECT name FROM t", &[]).unwrap();
        let all_v: Value = serde_json::from_str(&all_s).unwrap();
        assert_eq!(all_v.as_array().unwrap().len(), 1);

        let missing = get(&dir, "SELECT name FROM t WHERE id = 99", &[]).unwrap();
        assert_eq!(missing, "null");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrates_store_json_into_kv() {
        let dir = temp_plugin_dir();
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(
            dir.join("data").join("store.json"),
            "{\n  \"hello\": \"world\"\n}",
        )
        .unwrap();
        let row = get(
            &dir,
            "SELECT value FROM kv WHERE key = ?1",
            &[json!("hello")],
        )
        .unwrap();
        let row_v: Value = serde_json::from_str(&row).unwrap();
        assert_eq!(row_v["value"], "\"world\"");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
