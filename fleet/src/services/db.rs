//! The coordinator's records in one SQLite file (`fleet.db`): every state change, wake,
//! shutdown and approval (events), machine facts every 30 s (samples, kept 14 days), the
//! machines' power states and the jobs. Rows hold JSON: the shapes live in the code.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;
use std::sync::Mutex;

pub struct Db(Mutex<Connection>);

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub id: i64,
    pub at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    pub kind: String,
    pub detail: String,
}

/// How long events and samples are kept.
pub const KEEP_MS: u64 = 14 * 86_400_000;

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let c = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        c.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS events (
                 id INTEGER PRIMARY KEY, at INTEGER NOT NULL, device TEXT, job TEXT,
                 kind TEXT NOT NULL, detail TEXT NOT NULL);
             CREATE INDEX IF NOT EXISTS events_at ON events(at);
             CREATE TABLE IF NOT EXISTS samples (
                 at INTEGER NOT NULL, device TEXT NOT NULL, facts TEXT NOT NULL);
             CREATE INDEX IF NOT EXISTS samples_device_at ON samples(device, at);
             CREATE TABLE IF NOT EXISTS devices (id TEXT PRIMARY KEY, state TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS jobs (
                 id TEXT PRIMARY KEY, created_at INTEGER NOT NULL, state TEXT NOT NULL,
                 data TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS workflows (
                 id TEXT PRIMARY KEY, created_at INTEGER NOT NULL, data TEXT NOT NULL);",
        )?;
        Ok(Self(Mutex::new(c)))
    }

    fn c(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn event(
        &self,
        at: u64,
        device: Option<&str>,
        job: Option<&str>,
        kind: &str,
        detail: &str,
    ) {
        let r = self.c().execute(
            "INSERT INTO events (at, device, job, kind, detail) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![at as i64, device, job, kind, detail],
        );
        if let Err(e) = r {
            tracing::warn!("cannot record an event: {e}");
        }
    }

    /// The newest events first, optionally of one machine or one job.
    pub fn events(&self, device: Option<&str>, job: Option<&str>, limit: usize) -> Vec<Event> {
        let c = self.c();
        let mut q = c
            .prepare(
                "SELECT id, at, device, job, kind, detail FROM events
                 WHERE (?1 IS NULL OR device = ?1) AND (?2 IS NULL OR job = ?2)
                 ORDER BY id DESC LIMIT ?3",
            )
            .expect("events query");
        q.query_map(params![device, job, limit as i64], |r| {
            Ok(Event {
                id: r.get(0)?,
                at: r.get::<_, i64>(1)? as u64,
                device: r.get(2)?,
                job: r.get(3)?,
                kind: r.get(4)?,
                detail: r.get(5)?,
            })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    pub fn sample(&self, at: u64, device: &str, facts: &serde_json::Value) {
        let _ = self.c().execute(
            "INSERT INTO samples (at, device, facts) VALUES (?1, ?2, ?3)",
            params![at as i64, device, facts.to_string()],
        );
    }

    /// A machine's facts since `since` (oldest first).
    pub fn samples(&self, device: &str, since: u64) -> Vec<(u64, serde_json::Value)> {
        let c = self.c();
        let mut q = c
            .prepare("SELECT at, facts FROM samples WHERE device = ?1 AND at >= ?2 ORDER BY at")
            .expect("samples query");
        q.query_map(params![device, since as i64], |r| {
            let at: i64 = r.get(0)?;
            let facts: String = r.get(1)?;
            Ok((at as u64, serde_json::from_str(&facts).unwrap_or_default()))
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    /// Forget events and samples older than [`KEEP_MS`].
    pub fn prune(&self, now: u64) {
        let before = now.saturating_sub(KEEP_MS) as i64;
        let c = self.c();
        let _ = c.execute("DELETE FROM samples WHERE at < ?1", params![before]);
        let _ = c.execute("DELETE FROM events WHERE at < ?1", params![before]);
    }

    pub fn put_device<T: Serialize>(&self, id: &str, state: &T) {
        let _ = self.c().execute(
            "INSERT INTO devices (id, state) VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET state = excluded.state",
            params![id, serde_json::to_string(state).unwrap_or_default()],
        );
    }

    pub fn device_states<T: serde::de::DeserializeOwned>(&self) -> Vec<(String, T)> {
        let c = self.c();
        let mut q = c
            .prepare("SELECT id, state FROM devices")
            .expect("devices query");
        q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map(|rows| {
                rows.flatten()
                    .filter_map(|(id, s)| Some((id, serde_json::from_str(&s).ok()?)))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn put_job<T: Serialize>(
        &self,
        id: &str,
        created_at: u64,
        state: &str,
        job: &T,
    ) -> Result<()> {
        self.c().execute(
            "INSERT INTO jobs (id, created_at, state, data) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET state = excluded.state, data = excluded.data",
            params![id, created_at as i64, state, serde_json::to_string(job)?],
        )?;
        Ok(())
    }

    pub fn jobs<T: serde::de::DeserializeOwned>(&self) -> Vec<T> {
        let c = self.c();
        let mut q = c
            .prepare("SELECT data FROM jobs ORDER BY created_at, id")
            .expect("jobs query");
        q.query_map([], |r| r.get::<_, String>(0))
            .map(|rows| {
                rows.flatten()
                    .filter_map(|d| serde_json::from_str(&d).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn delete_job(&self, id: &str) {
        let _ = self
            .c()
            .execute("DELETE FROM jobs WHERE id = ?1", params![id]);
    }

    pub fn put_workflow<T: Serialize>(&self, id: &str, created_at: u64, w: &T) -> Result<()> {
        self.c().execute(
            "INSERT INTO workflows (id, created_at, data) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET data = excluded.data",
            params![id, created_at as i64, serde_json::to_string(w)?],
        )?;
        Ok(())
    }

    pub fn workflows<T: serde::de::DeserializeOwned>(&self) -> Vec<T> {
        let c = self.c();
        let mut q = c
            .prepare("SELECT data FROM workflows ORDER BY created_at, id")
            .expect("workflows query");
        q.query_map([], |r| r.get::<_, String>(0))
            .map(|rows| {
                rows.flatten()
                    .filter_map(|d| serde_json::from_str(&d).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A consistent copy of the database (backups), safe while it is in use.
    pub fn backup_to(&self, path: &Path) -> Result<()> {
        let c = self.c();
        let _ = std::fs::remove_file(path);
        c.execute("VACUUM INTO ?1", params![path.display().to_string()])?;
        Ok(())
    }

    pub fn has_job(&self, id: &str) -> bool {
        self.c()
            .query_row("SELECT 1 FROM jobs WHERE id = ?1", params![id], |_| Ok(()))
            .optional()
            .ok()
            .flatten()
            .is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_samples_states_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("fleet.db")).unwrap();
        db.event(1, Some("nuc"), None, "power", "waking");
        db.event(2, Some("pi5"), Some("j-1"), "job", "running");
        db.event(3, Some("nuc"), None, "power", "ready");
        let nuc = db.events(Some("nuc"), None, 10);
        assert_eq!(nuc.len(), 2);
        assert_eq!(nuc[0].detail, "ready");
        assert_eq!(db.events(None, Some("j-1"), 10).len(), 1);
        db.sample(10, "nuc", &serde_json::json!({ "load": [1.0, 0.5, 0.2] }));
        assert_eq!(db.samples("nuc", 0)[0].1["load"][0], 1.0);
        db.prune(10 + KEEP_MS + 1);
        assert!(db.samples("nuc", 0).is_empty());
        assert!(db.events(None, None, 10).is_empty());
        db.put_device("nuc", &serde_json::json!({ "power": "off" }));
        db.put_device("nuc", &serde_json::json!({ "power": "ready" }));
        let states: Vec<(String, serde_json::Value)> = db.device_states();
        assert_eq!(
            states,
            vec![("nuc".into(), serde_json::json!({ "power": "ready" }))]
        );
        db.put_job("j-1", 5, "ready", &serde_json::json!({ "id": "j-1" }))
            .unwrap();
        assert!(db.has_job("j-1"));
        let jobs: Vec<serde_json::Value> = db.jobs();
        assert_eq!(jobs.len(), 1);
        db.delete_job("j-1");
        assert!(!db.has_job("j-1"));
        db.backup_to(&dir.path().join("copy.db")).unwrap();
        assert!(dir.path().join("copy.db").exists());
    }
}
