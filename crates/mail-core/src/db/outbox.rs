use super::{Db, OutboxSummaryRow};
use anyhow::Result;
use rusqlite::{params, OptionalExtension};

/// A message held in the outbox.
pub struct OutboxRow {
    pub id: i64,
    pub account_id: i64,
    pub message_id: String,
    pub payload: String,
    pub raw: Vec<u8>,
    pub envelope: String,
    pub attempts: i64,
}

/// A snooze whose time has come.
pub struct DueSnooze {
    pub id: i64,
    pub account_id: i64,
    pub message_id: String,
    pub return_path: String,
}


impl Db {
    // ---------- outbox ----------

    pub fn add_outbox(&self, account_id: i64, message_id: &str, payload: &str, raw: &[u8], envelope: &str, send_at: i64) -> Result<i64> {
        let c = self.c();
        c.execute(
            "INSERT INTO outbox (account_id, message_id, payload, raw, envelope, send_at) VALUES (?, ?, ?, ?, ?, ?)",
            params![account_id, message_id, payload, raw, envelope, send_at],
        )?;
        Ok(c.last_insert_rowid())
    }

    /// (id, account_id, payload, send_at, error) for everything in the outbox, soonest first.
    pub fn outbox(&self) -> Result<Vec<OutboxSummaryRow>> {
        let c = self.r();
        let rows = c
            .prepare("SELECT id, account_id, payload, send_at, error FROM outbox ORDER BY send_at, id")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    fn outbox_row(r: &rusqlite::Row) -> rusqlite::Result<OutboxRow> {
        Ok(OutboxRow {
            id: r.get(0)?,
            account_id: r.get(1)?,
            message_id: r.get(2)?,
            payload: r.get(3)?,
            raw: r.get(4)?,
            envelope: r.get(5)?,
            attempts: r.get(6)?,
        })
    }

    /// Claims every message due by `now` for sending. Claimed ones can't be cancelled.
    pub fn claim_due_outbox(&self, now: i64) -> Result<Vec<OutboxRow>> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let rows: Vec<OutboxRow> = tx
            .prepare("SELECT id, account_id, message_id, payload, raw, envelope, attempts FROM outbox WHERE sending = 0 AND send_at <= ?")?
            .query_map([now], Self::outbox_row)?
            .collect::<rusqlite::Result<_>>()?;
        for r in &rows {
            tx.execute("UPDATE outbox SET sending = 1 WHERE id = ?", [r.id])?;
        }
        tx.commit()?;
        Ok(rows)
    }

    /// Takes a message out of the outbox, unless it is being sent right now.
    pub fn take_outbox(&self, id: i64) -> Result<Option<OutboxRow>> {
        let c = self.c();
        let row = c
            .query_row(
                "SELECT id, account_id, message_id, payload, raw, envelope, attempts FROM outbox WHERE id = ? AND sending = 0",
                [id],
                Self::outbox_row,
            )
            .optional()?;
        if row.is_some() {
            c.execute("DELETE FROM outbox WHERE id = ?", [id])?;
        }
        Ok(row)
    }

    pub fn delete_outbox(&self, id: i64) -> Result<()> {
        self.c().execute("DELETE FROM outbox WHERE id = ?", [id])?;
        Ok(())
    }

    pub fn outbox_failed(&self, id: i64, error: &str, retry_at: i64) -> Result<()> {
        self.c().execute(
            "UPDATE outbox SET sending = 0, attempts = attempts + 1, error = ?, send_at = ? WHERE id = ?",
            params![error, retry_at, id],
        )?;
        Ok(())
    }

    pub fn reschedule_outbox(&self, id: i64, send_at: i64) -> Result<bool> {
        Ok(self.c().execute("UPDATE outbox SET send_at = ?, error = NULL WHERE id = ? AND sending = 0", params![send_at, id])? > 0)
    }

    /// After a crash nothing is mid-send any more.
    pub fn reset_outbox_claims(&self) -> Result<()> {
        self.c().execute("UPDATE outbox SET sending = 0 WHERE sending = 1", [])?;
        Ok(())
    }

    // ---------- snoozes ----------

    pub fn add_snooze(&self, account_id: i64, message_id: &str, return_path: &str, wake_at: i64) -> Result<()> {
        self.c().execute(
            "INSERT INTO snoozes (account_id, message_id, return_path, wake_at) VALUES (?, ?, ?, ?)
             ON CONFLICT(account_id, message_id) DO UPDATE SET wake_at = excluded.wake_at, woke_at = NULL,
               return_path = CASE WHEN snoozes.woke_at IS NULL THEN snoozes.return_path ELSE excluded.return_path END",
            params![account_id, message_id, return_path, wake_at],
        )?;
        Ok(())
    }

    /// Moves a pending snooze's wake time (e.g. to now, to unsnooze).
    pub fn set_wake(&self, account_id: i64, message_id: &str, wake_at: i64) -> Result<bool> {
        Ok(self.c().execute(
            "UPDATE snoozes SET wake_at = ? WHERE account_id = ? AND message_id = ? AND woke_at IS NULL",
            params![wake_at, account_id, message_id],
        )? > 0)
    }

    pub fn due_snoozes(&self, now: i64) -> Result<Vec<DueSnooze>> {
        let c = self.r();
        let rows = c
            .prepare("SELECT id, account_id, message_id, return_path FROM snoozes WHERE woke_at IS NULL AND wake_at <= ?")?
            .query_map([now], |r| Ok(DueSnooze { id: r.get(0)?, account_id: r.get(1)?, message_id: r.get(2)?, return_path: r.get(3)? }))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn mark_woke(&self, id: i64, at: i64) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        tx.prepare_cached("UPDATE snoozes SET woke_at = ? WHERE id = ?")?.execute(params![at, id])?;
        if let Some((account_id, message_id)) = tx
            .prepare_cached("SELECT account_id, message_id FROM snoozes WHERE id = ?")?
            .query_row([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .optional()?
        {
            tx.prepare_cached(
                "UPDATE messages SET sort_date = MAX(sort_date, ?1) WHERE account_id = ?2 AND message_id = ?3",
            )?
            .execute(params![at, account_id, message_id])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The soonest moment the scheduler has work: an outbox message or a snooze coming due.
    pub fn next_due(&self) -> Result<Option<i64>> {
        let c = self.r();
        Ok(c.query_row(
            "SELECT MIN(t) FROM (SELECT MIN(send_at) AS t FROM outbox WHERE sending = 0
                                 UNION ALL SELECT MIN(wake_at) FROM snoozes WHERE woke_at IS NULL)",
            [],
            |r| r.get(0),
        )?)
    }

}
