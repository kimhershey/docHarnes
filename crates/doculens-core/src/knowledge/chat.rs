//! Assistant chat sessions (0.7.0 item 6, `docs/plan/v0.7.0.md` §1.6): the AI assistant's
//! conversations live per workspace in `chat_sessions` / `chat_messages` (migration 0012). A
//! message's `payload` is the UI's own JSON (user text, plan / answer / result cards …) and is
//! stored as is — the core only derives the automatic session title from the first user message
//! and expires sessions with the "대화 기록" retention item.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::documents::new_id;

/// Characters of the first user message that become the automatic title.
pub const TITLE_CHARS: usize = 40;
/// Longest title a rename may set.
pub const MAX_TITLE_CHARS: usize = 120;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatSessionSummary {
    pub id: String,
    pub title: String,
    /// The title came from the first message (a rename clears it).
    pub title_auto: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub message_count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessageRow {
    pub seq: i64,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn parse_ts(s: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now())
}

const SESSION_COLS: &str = "s.id, s.title, s.title_auto, s.created_at, s.updated_at, \
     (SELECT COUNT(*) FROM chat_messages m WHERE m.session_id = s.id)";

fn session_from_row(r: &Row<'_>) -> rusqlite::Result<ChatSessionSummary> {
    Ok(ChatSessionSummary {
        id: r.get(0)?,
        title: r.get(1)?,
        title_auto: r.get::<_, i64>(2)? != 0,
        created_at: parse_ts(r.get(3)?),
        updated_at: parse_ts(r.get(4)?),
        message_count: r.get::<_, i64>(5)?.max(0) as u32,
    })
}

/// Sessions, most recently updated first.
pub fn list_sessions(conn: &Connection) -> rusqlite::Result<Vec<ChatSessionSummary>> {
    let mut st = conn.prepare(&format!(
        "SELECT {SESSION_COLS} FROM chat_sessions s ORDER BY s.updated_at DESC, s.created_at DESC, s.id"
    ))?;
    let rows = st.query_map([], session_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_session(conn: &Connection, id: &str) -> rusqlite::Result<Option<ChatSessionSummary>> {
    conn.query_row(&format!("SELECT {SESSION_COLS} FROM chat_sessions s WHERE s.id = ?1"), [id], session_from_row)
        .optional()
}

/// Trims, collapses whitespace and clips a title.
fn clean_title(title: &str, max: usize) -> String {
    let joined = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = joined.chars().take(max).collect();
    if joined.chars().count() > max {
        out.push('…');
    }
    out
}

/// Creates a session; a given title is kept verbatim (not automatic), otherwise the first user
/// message names it.
pub fn create_session(conn: &Connection, title: Option<&str>) -> rusqlite::Result<ChatSessionSummary> {
    let id = new_id();
    let title = title.map(|t| clean_title(t, MAX_TITLE_CHARS)).filter(|t| !t.is_empty());
    let ts = now();
    conn.execute(
        "INSERT INTO chat_sessions (id, title, title_auto, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
        params![id, title.clone().unwrap_or_default(), title.is_none() as i64, ts],
    )?;
    Ok(ChatSessionSummary {
        id,
        title: title.unwrap_or_default(),
        title_auto: true,
        created_at: parse_ts(ts.clone()),
        updated_at: parse_ts(ts),
        message_count: 0,
    })
    .map(|mut s| {
        s.title_auto = s.title.is_empty();
        s
    })
}

/// Renames a session (an empty title restores the automatic one). `false` when it does not exist.
pub fn rename_session(conn: &Connection, id: &str, title: &str) -> rusqlite::Result<bool> {
    let title = clean_title(title, MAX_TITLE_CHARS);
    let n = if title.is_empty() {
        let auto = first_user_text(conn, id)?.map(|t| clean_title(&t, TITLE_CHARS)).unwrap_or_default();
        conn.execute(
            "UPDATE chat_sessions SET title = ?2, title_auto = 1, updated_at = ?3 WHERE id = ?1",
            params![id, auto, now()],
        )?
    } else {
        conn.execute(
            "UPDATE chat_sessions SET title = ?2, title_auto = 0, updated_at = ?3 WHERE id = ?1",
            params![id, title, now()],
        )?
    };
    Ok(n > 0)
}

/// Deletes a session and its messages (cascade). `false` when it does not exist.
pub fn delete_session(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM chat_sessions WHERE id = ?1", [id])? > 0)
}

/// The messages of a session in order.
pub fn list_messages(conn: &Connection, session_id: &str) -> rusqlite::Result<Vec<ChatMessageRow>> {
    let mut st =
        conn.prepare("SELECT seq, payload, created_at FROM chat_messages WHERE session_id = ?1 ORDER BY seq")?;
    let rows = st
        .query_map([session_id], |r| {
            let payload: String = r.get(1)?;
            Ok(ChatMessageRow {
                seq: r.get(0)?,
                payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
                created_at: parse_ts(r.get(2)?),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The text of a user message payload (`{"role":"user","text":…}`), if it is one.
pub fn user_text(payload: &Value) -> Option<&str> {
    (payload.get("role").and_then(Value::as_str) == Some("user"))
        .then(|| payload.get("text").and_then(Value::as_str))
        .flatten()
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

fn first_user_text(conn: &Connection, session_id: &str) -> rusqlite::Result<Option<String>> {
    let mut st = conn.prepare("SELECT payload FROM chat_messages WHERE session_id = ?1 ORDER BY seq")?;
    let rows = st.query_map([session_id], |r| r.get::<_, String>(0))?;
    for row in rows {
        let payload: Value = serde_json::from_str(&row?).unwrap_or(Value::Null);
        if let Some(t) = user_text(&payload) {
            return Ok(Some(t.to_string()));
        }
    }
    Ok(None)
}

/// Stores (or replaces) message `seq` of a session, bumps `updated_at` and names an untitled
/// session after its first user message. A session the retention purge removed while the UI
/// still had it open is re-created (untitled) so the conversation is not dropped silently;
/// `false` only for an empty session id.
pub fn put_message(conn: &Connection, session_id: &str, seq: i64, payload: &Value) -> rusqlite::Result<bool> {
    if session_id.trim().is_empty() {
        return Ok(false);
    }
    let ts = now();
    let session = match get_session(conn, session_id)? {
        Some(s) => s,
        None => {
            conn.execute(
                "INSERT INTO chat_sessions (id, title, title_auto, created_at, updated_at) VALUES (?1, '', 1, ?2, ?2)",
                params![session_id, ts],
            )?;
            ChatSessionSummary {
                id: session_id.to_string(),
                title: String::new(),
                title_auto: true,
                created_at: parse_ts(ts.clone()),
                updated_at: parse_ts(ts.clone()),
                message_count: 0,
            }
        }
    };
    conn.execute(
        "INSERT INTO chat_messages (session_id, seq, payload, created_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(session_id, seq) DO UPDATE SET payload = excluded.payload",
        params![session_id, seq, payload.to_string(), ts],
    )?;
    let auto_title = (session.title_auto && session.title.is_empty())
        .then(|| user_text(payload).map(|t| clean_title(t, TITLE_CHARS)))
        .flatten();
    match auto_title {
        Some(title) => conn.execute(
            "UPDATE chat_sessions SET title = ?2, updated_at = ?3 WHERE id = ?1",
            params![session_id, title, ts],
        )?,
        None => conn.execute("UPDATE chat_sessions SET updated_at = ?2 WHERE id = ?1", params![session_id, ts])?,
    };
    Ok(true)
}

/// Removes the messages of a session from `seq` on (a conversation cut short by the user).
pub fn truncate_messages(conn: &Connection, session_id: &str, from_seq: i64) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM chat_messages WHERE session_id = ?1 AND seq >= ?2", params![session_id, from_seq])
}

/// Deletes sessions not touched since `cutoff` (retention purge, "대화 기록"); messages cascade.
pub fn purge_sessions_before(conn: &Connection, cutoff: &DateTime<Utc>) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM chat_sessions WHERE updated_at < ?1", [cutoff.to_rfc3339()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use serde_json::json;

    #[test]
    fn sessions_messages_titles_and_purge() {
        let db = Db::open_in_memory().unwrap();
        let s = db.with(|c| create_session(c, None)).unwrap();
        assert!(s.title_auto && s.title.is_empty() && s.message_count == 0);
        // The first user message names the session (clipped to 40 characters), later ones do not.
        let long = "계약서 세 건의 공급가액 합계를 알려줘 그리고 가장 큰 건은 어느 것인지도 같이 정리해 줘";
        assert!(db.with(|c| put_message(c, &s.id, 1, &json!({"id": 1, "role": "user", "text": long}))).unwrap());
        assert!(db.with(|c| put_message(c, &s.id, 2, &json!({"id": 2, "role": "assistant", "kind": "help"}))).unwrap());
        assert!(db.with(|c| put_message(c, &s.id, 3, &json!({"id": 3, "role": "user", "text": "두 번째"}))).unwrap());
        let got = db.with(|c| get_session(c, &s.id)).unwrap().unwrap();
        assert_eq!(got.title.chars().count(), TITLE_CHARS + 1, "{}", got.title);
        assert!(got.title.ends_with('…') && got.title.starts_with("계약서 세 건"));
        assert!(got.title_auto);
        assert_eq!(got.message_count, 3);
        // Upsert replaces the payload of an existing seq.
        assert!(db
            .with(|c| put_message(
                c,
                &s.id,
                2,
                &json!({"id": 2, "role": "assistant", "kind": "answer", "pending": false})
            ))
            .unwrap());
        let rows = db.with(|c| list_messages(c, &s.id)).unwrap();
        assert_eq!(rows.iter().map(|r| r.seq).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(rows[1].payload["kind"], "answer");
        // A session the purge removed is re-created by the next message (untitled → auto title); an empty id is refused.
        assert!(db.with(|c| put_message(c, "gone", 1, &json!({"role": "user", "text": "되살림"}))).unwrap());
        let revived = db.with(|c| get_session(c, "gone")).unwrap().unwrap();
        assert_eq!((revived.title.as_str(), revived.message_count), ("되살림", 1));
        assert!(db.with(|c| delete_session(c, "gone")).unwrap());
        assert!(!db.with(|c| put_message(c, "", 1, &json!({"role": "user", "text": "x"}))).unwrap());
        // Rename clears the automatic flag; an empty rename restores the automatic title.
        assert!(db.with(|c| rename_session(c, &s.id, "  합계   정리 ")).unwrap());
        let got = db.with(|c| get_session(c, &s.id)).unwrap().unwrap();
        assert_eq!((got.title.as_str(), got.title_auto), ("합계 정리", false));
        assert!(db.with(|c| rename_session(c, &s.id, "")).unwrap());
        let got = db.with(|c| get_session(c, &s.id)).unwrap().unwrap();
        assert!(got.title_auto && got.title.starts_with("계약서 세 건"));
        // A second, titled session sorts first (most recently updated).
        let t = db.with(|c| create_session(c, Some("검토"))).unwrap();
        assert!(!t.title_auto);
        let list = db.with(list_sessions).unwrap();
        assert_eq!(list.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), [t.id.as_str(), s.id.as_str()]);
        // Truncate, delete (cascade) and purge.
        assert_eq!(db.with(|c| truncate_messages(c, &s.id, 3)).unwrap(), 1);
        assert_eq!(db.with(|c| list_messages(c, &s.id)).unwrap().len(), 2);
        assert!(db.with(|c| delete_session(c, &t.id)).unwrap());
        assert!(!db.with(|c| delete_session(c, &t.id)).unwrap());
        let future = Utc::now() + chrono::Duration::days(1);
        assert_eq!(db.with(|c| purge_sessions_before(c, &future)).unwrap(), 1);
        let left: i64 = db.with(|c| c.query_row("SELECT COUNT(*) FROM chat_messages", [], |r| r.get(0))).unwrap();
        assert_eq!(left, 0, "messages cascade with their session");
    }
}
