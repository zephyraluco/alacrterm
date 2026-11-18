//! Product metadata and UI event projection. Model history belongs solely to
//! Rig's native memory store, never reconstructed from these display events.
use crate::types::{Event, EventKind, RunInput, Session};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

#[derive(Clone)]
pub struct ChatStore(Arc<Mutex<Connection>>);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as u64
}

impl ChatStore {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch("PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS chat_sessions (id TEXT PRIMARY KEY, metadata TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS chat_events (session TEXT NOT NULL, sequence INTEGER NOT NULL, event TEXT NOT NULL, PRIMARY KEY(session,sequence));")?;
        Ok(Self(Arc::new(Mutex::new(connection))))
    }

    pub fn create(&self) -> anyhow::Result<Session> {
        let session = Session {
            id: Uuid::new_v4(),
            title: "新对话".into(),
            model: None,
            target: None,
            updated_ms: now_ms(),
            active_run: None,
        };
        self.0
            .lock()
            .map_err(|_| anyhow::anyhow!("session database unavailable"))?
            .execute(
                "INSERT INTO chat_sessions(id,metadata) VALUES (?1,?2)",
                params![session.id.to_string(), serde_json::to_string(&session)?],
            )?;
        Ok(session)
    }

    pub fn list(&self) -> anyhow::Result<Vec<Session>> {
        let connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("session database unavailable"))?;
        let mut statement = connection.prepare("SELECT metadata FROM chat_sessions")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(serde_json::from_str::<Session>(&row?)?);
        }
        sessions.sort_by_key(|s| std::cmp::Reverse(s.updated_ms));
        Ok(sessions)
    }

    pub fn events(&self, session: Uuid, after: u64) -> anyhow::Result<Vec<Event>> {
        let connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("session database unavailable"))?;
        let mut statement = connection.prepare(
            "SELECT event FROM chat_events WHERE session=?1 AND sequence>?2 ORDER BY sequence",
        )?;
        let rows = statement.query_map(params![session.to_string(), after], |row| {
            row.get::<_, String>(0)
        })?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    pub fn begin(&self, id: Uuid, run: Uuid, input: RunInput) -> anyhow::Result<Event> {
        let RunInput {
            model,
            target,
            prompt,
            mode,
        } = input;
        let mut connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("session database unavailable"))?;
        let transaction = connection.transaction()?;
        let mut session = load(&transaction, id)?;
        anyhow::ensure!(
            session.active_run.is_none(),
            "session already has an active run"
        );
        session.active_run = Some(run);
        session.model = Some(model);
        session.target = target.clone();
        if session.title == "新对话" {
            session.title = prompt
                .lines()
                .next()
                .unwrap_or("新对话")
                .chars()
                .take(48)
                .collect();
        }
        let event = write(
            &transaction,
            &mut session,
            run,
            EventKind::Started {
                prompt,
                target,
                mode,
            },
        )?;
        transaction.commit()?;
        Ok(event)
    }

    pub fn append(&self, id: Uuid, run: Uuid, kind: EventKind) -> anyhow::Result<Event> {
        let mut connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("session database unavailable"))?;
        let transaction = connection.transaction()?;
        let mut session = load(&transaction, id)?;
        anyhow::ensure!(session.active_run == Some(run), "stale or finished run");
        if kind.is_terminal() {
            session.active_run = None;
        }
        let event = write(&transaction, &mut session, run, kind)?;
        transaction.commit()?;
        Ok(event)
    }

    /// Called once by the application owner at startup. Incomplete runs remain
    /// visible as interrupted; reopening a window must not call this method.
    pub fn recover(&self) -> anyhow::Result<()> {
        for session in self.list()? {
            if let Some(run) = session.active_run {
                self.append(
                    session.id,
                    run,
                    EventKind::Failed {
                        message: "Previous run was interrupted when the application stopped."
                            .into(),
                    },
                )?;
            }
        }
        Ok(())
    }
}

fn load(connection: &Connection, id: Uuid) -> anyhow::Result<Session> {
    let value: String = connection
        .query_row(
            "SELECT metadata FROM chat_sessions WHERE id=?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("session does not exist"))?;
    Ok(serde_json::from_str(&value)?)
}

fn write(
    connection: &Connection,
    session: &mut Session,
    run: Uuid,
    kind: EventKind,
) -> anyhow::Result<Event> {
    let sequence: u64 = connection.query_row(
        "SELECT COALESCE(MAX(sequence),0)+1 FROM chat_events WHERE session=?1",
        [session.id.to_string()],
        |row| row.get(0),
    )?;
    let event = Event {
        session: session.id,
        run,
        sequence,
        kind,
    };
    session.updated_ms = now_ms();
    connection.execute(
        "INSERT INTO chat_events(session,sequence,event) VALUES (?1,?2,?3)",
        params![
            session.id.to_string(),
            sequence,
            serde_json::to_string(&event)?
        ],
    )?;
    connection.execute(
        "UPDATE chat_sessions SET metadata=?2 WHERE id=?1",
        params![session.id.to_string(), serde_json::to_string(session)?],
    )?;
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Mode;
    #[test]
    fn persists_interruption_and_rejects_stale_events_and_overlapping_runs() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("chat.sqlite");
        let store = ChatStore::open(&path).unwrap();
        let session = store.create().unwrap();
        let first = Uuid::new_v4();
        let model = Uuid::new_v4();
        let input = |prompt: &str| RunInput {
            model,
            target: None,
            prompt: prompt.into(),
            mode: Mode::Ask,
        };
        store.begin(session.id, first, input("hello")).unwrap();
        assert!(
            store
                .begin(session.id, Uuid::new_v4(), input("overlap"))
                .is_err()
        );
        store
            .append(
                session.id,
                first,
                EventKind::TextDelta {
                    text: "partial".into(),
                },
            )
            .unwrap();
        drop(store);
        let store = ChatStore::open(&path).unwrap();
        store.recover().unwrap();
        assert!(store.list().unwrap()[0].active_run.is_none());
        assert!(
            store
                .append(session.id, first, EventKind::Completed)
                .is_err()
        );
        let second = Uuid::new_v4();
        store.begin(session.id, second, input("continue")).unwrap();
        store
            .append(session.id, second, EventKind::Completed)
            .unwrap();
        let events = store.events(session.id, 0).unwrap();
        assert_eq!(events.len(), 5);
        assert!(
            events
                .iter()
                .enumerate()
                .all(|(i, e)| e.sequence == i as u64 + 1)
        );
        assert_eq!(store.events(session.id, 3).unwrap().len(), 2);
    }
}
