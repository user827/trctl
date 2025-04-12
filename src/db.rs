use std::{path::PathBuf, time::SystemTime};

use rusqlite::Connection;

use crate::errors::*;

use log::debug;

pub struct DBSqlite {
    conn: Option<Connection>,
    path: Option<PathBuf>,
}

impl DBSqlite {
    #[must_use]
    pub fn new(path: Option<PathBuf>) -> Self {
        Self { conn: None, path }
    }

    fn init(&mut self) -> Result<&mut Connection> {
        debug!("initializing db");
        if let Some(ref mut c) = self.conn {
            debug!("already init");
            return Ok(c);
        }

        let conn = Connection::open(self.path.as_ref().unwrap())?;
        let mut statement = conn.prepare(
            "
            SELECT name FROM sqlite_master WHERE type='table' AND name='torrents';
            ",
        )?;
        if statement.query([])?.next()?.is_none() {
            Self::create_tables(&conn)?;
        }
        drop(statement);

        self.conn = Some(conn);
        Ok(self.conn.as_mut().unwrap())
    }

    fn create_tables(conn: &Connection) -> Result<()> {
        debug!("create tables");
        let query = "
        CREATE TABLE torrents (hash TEXT PRIMARY KEY, timestamp BIGINT);
        ";
        conn.execute(query, [])?;
        Ok(())
    }
}

pub trait DB {
    fn store(&mut self, hsh: &str) -> Result<()>;
    fn has(&mut self, hsh: &str) -> Result<Option<u64>>;
}

impl DB for DBSqlite {
    fn store(&mut self, hsh: &str) -> Result<()> {
        if self.path.is_none() {
            debug!("not enabled");
            return Ok(());
        }
        let conn = self.init()?;
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_secs();
        conn.execute(
            "INSERT OR IGNORE INTO torrents (hash, timestamp) VALUES (?1, ?2);",
            (hsh, timestamp),
        )?;
        Ok(())
    }

    fn has(&mut self, hsh: &str) -> Result<Option<u64>> {
        if self.path.is_none() {
            debug!("not enabled");
            return Ok(None);
        }
        let conn = self.init()?;

        let mut statement = conn.prepare(
            "
            SELECT timestamp FROM torrents WHERE hash = ?1;
            ",
        )?;
        if let Some(res) = statement.query([hsh])?.next()? {
            return Ok(res.get(0)?);
        }
        Ok(None)
    }
}
