//! Durable, resumable sessions.
//!
//! Messages, tool calls, model selection, changed files, pending tasks, and
//! the current plan/goal are persisted so a session survives crashes. Tool
//! calls carry an idempotency key so recovery does not repeat completed side
//! effects.

use nexus_core::ids::SessionId;
use nexus_core::store::Store;
use nexus_core::{NexusError, Result};
use nexus_models::types::{ChatMessage, Role};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: SessionId,
    pub title: String,
    pub workspace: String,
    pub created_at: String,
    pub updated_at: String,
    pub model: String,
    pub agent: String,
    pub summary: String,
    pub pending_tasks: Vec<String>,
    pub changed_files: Vec<String>,
    pub current_goal: Option<String>,
    pub status: String,
    pub persona_id: Option<String>,
    pub profile_name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionUsage {
    pub session_id: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub tool_calls: u64,
    pub elapsed_ms: u64,
    pub started_at: String,
    pub updated_at: String,
    pub exit_at: Option<String>,
}

#[derive(Clone)]
pub struct SessionStore {
    store: Store,
}

impl SessionStore {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub fn create(&self, workspace: &str, agent: &str, model: &str) -> Result<SessionId> {
        let id = SessionId::generate();
        let now = nexus_core::now_rfc3339();
        self.store.with(|conn| {
            conn.execute(
                "INSERT INTO sessions (id, title, workspace, created_at, updated_at, model, agent, status)
                 VALUES (?1, '', ?2, ?3, ?3, ?4, ?5, 'active')",
                rusqlite::params![id.as_str(), workspace, now, model, agent],
            )?;
            Ok(())
        })?;
        Ok(id)
    }

    pub fn get(&self, id: &str) -> Result<SessionMeta> {
        self.store.with(|conn| {
            conn.query_row(
                "SELECT id, title, workspace, created_at, updated_at, model, agent, summary,
                        pending_tasks, changed_files, current_goal, status, persona_id, profile_name
                 FROM sessions WHERE id = ?1",
                [id],
                row_to_meta,
            )
            .map_err(|_| NexusError::NotFound(format!("session `{id}`")))
        })
    }

    pub fn list(&self, workspace: Option<&str>, limit: usize) -> Result<Vec<SessionMeta>> {
        self.store.with(|conn| {
            let (sql, params): (String, Vec<String>) = match workspace {
                Some(ws) => (
                    "SELECT id, title, workspace, created_at, updated_at, model, agent, summary,
                            pending_tasks, changed_files, current_goal, status, persona_id, profile_name
                     FROM sessions WHERE workspace = ?1 ORDER BY updated_at DESC LIMIT ?2"
                        .into(),
                    vec![ws.to_string(), limit.to_string()],
                ),
                None => (
                    "SELECT id, title, workspace, created_at, updated_at, model, agent, summary,
                            pending_tasks, changed_files, current_goal, status, persona_id, profile_name
                     FROM sessions ORDER BY updated_at DESC LIMIT ?1"
                        .into(),
                    vec![limit.to_string()],
                ),
            };
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(params), row_to_meta)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
    }

    /// Append a message to the session at the given turn.
    pub fn add_message(&self, id: &str, turn: i64, message: &ChatMessage) -> Result<()> {
        let role = role_str(message.role);
        let tool_call_id = message.tool_call_id.clone();
        let tool_name = message.name.clone();
        let content = if message.tool_calls.is_empty() {
            message.content.clone()
        } else {
            // Store tool calls inline as JSON so history reload is lossless.
            serde_json::json!({
                "text": message.content,
                "tool_calls": message.tool_calls.iter().map(|c| serde_json::json!({
                    "id": c.id, "name": c.name, "arguments": c.arguments
                })).collect::<Vec<_>>()
            })
            .to_string()
        };
        self.store.with(|conn| {
            conn.execute(
                "INSERT INTO messages (session_id, turn, role, content, tool_call_id, tool_name, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                rusqlite::params![id, turn, role, content, tool_call_id, tool_name, nexus_core::now_rfc3339()],
            )?;
            conn.execute(
                "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
                rusqlite::params![nexus_core::now_rfc3339(), id],
            )?;
            Ok(())
        })
    }

    /// Load the full message history for a session, reconstructing tool calls.
    pub fn messages(&self, id: &str) -> Result<Vec<ChatMessage>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT role, content, tool_call_id, tool_name FROM messages
                 WHERE session_id = ?1 ORDER BY id",
            )?;
            let rows = stmt.query_map([id], |row| {
                let role: String = row.get(0)?;
                let content: String = row.get(1)?;
                let tool_call_id: Option<String> = row.get(2)?;
                let tool_name: Option<String> = row.get(3)?;
                Ok(reconstruct_message(
                    &role,
                    &content,
                    tool_call_id,
                    tool_name,
                ))
            })?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
    }

    pub fn max_turn(&self, id: &str) -> Result<i64> {
        self.store.with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT COALESCE(MAX(turn), 0) FROM messages WHERE session_id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .unwrap_or(0))
        })
    }

    pub fn set_summary(&self, id: &str, summary: &str) -> Result<()> {
        self.store.with(|conn| {
            conn.execute(
                "UPDATE sessions SET summary = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![summary, nexus_core::now_rfc3339(), id],
            )?;
            Ok(())
        })
    }

    pub fn set_pending_tasks(&self, id: &str, tasks: &[String]) -> Result<()> {
        self.store.with(|conn| {
            conn.execute(
                "UPDATE sessions SET pending_tasks = ?1 WHERE id = ?2",
                rusqlite::params![serde_json::to_string(tasks)?, id],
            )?;
            Ok(())
        })
    }

    pub fn record_changed_file(&self, id: &str, path: &str) -> Result<()> {
        let meta = self.get(id)?;
        let mut files = meta.changed_files;
        if !files.iter().any(|f| f == path) {
            files.push(path.to_string());
        }
        self.store.with(|conn| {
            conn.execute(
                "UPDATE sessions SET changed_files = ?1 WHERE id = ?2",
                rusqlite::params![serde_json::to_string(&files)?, id],
            )?;
            Ok(())
        })
    }

    pub fn set_model(&self, id: &str, model: &str) -> Result<()> {
        self.store.with(|conn| {
            conn.execute(
                "UPDATE sessions SET model = ?1 WHERE id = ?2",
                rusqlite::params![model, id],
            )?;
            Ok(())
        })
    }

    pub fn set_status(&self, id: &str, status: &str) -> Result<()> {
        self.store.with(|conn| {
            let changed = conn.execute(
                "UPDATE sessions SET status=?1,updated_at=?2 WHERE id=?3",
                rusqlite::params![status, nexus_core::now_rfc3339(), id],
            )?;
            if changed == 0 {
                return Err(NexusError::NotFound(format!("session `{id}`")));
            }
            Ok(())
        })
    }

    pub fn set_agent(&self, id: &str, agent: &str) -> Result<()> {
        self.store.with(|conn| {
            let changed = conn.execute(
                "UPDATE sessions SET agent = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![agent, nexus_core::now_rfc3339(), id],
            )?;
            if changed == 0 {
                return Err(NexusError::NotFound(format!("session `{id}`")));
            }
            Ok(())
        })
    }

    pub fn set_current_goal(&self, id: &str, goal_id: Option<&str>) -> Result<()> {
        self.store.with(|conn| {
            let n = conn.execute(
                "UPDATE sessions SET current_goal = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![goal_id, nexus_core::now_rfc3339(), id],
            )?;
            if n == 0 {
                return Err(NexusError::NotFound(format!("session `{id}`")));
            }
            Ok(())
        })
    }

    pub fn set_persona_profile(
        &self,
        id: &str,
        persona_id: Option<&str>,
        profile_name: &str,
    ) -> Result<()> {
        self.store.with(|conn| {
            let n = conn.execute(
                "UPDATE sessions SET persona_id = ?1, profile_name = ?2, updated_at = ?3
                 WHERE id = ?4",
                rusqlite::params![persona_id, profile_name, nexus_core::now_rfc3339(), id],
            )?;
            if n == 0 {
                return Err(NexusError::NotFound(format!("session `{id}`")));
            }
            Ok(())
        })
    }

    pub fn rename(&self, id: &str, title: &str) -> Result<()> {
        self.store.with(|conn| {
            let n = conn.execute(
                "UPDATE sessions SET title = ?1 WHERE id = ?2",
                rusqlite::params![title, id],
            )?;
            if n == 0 {
                return Err(NexusError::NotFound(format!("session `{id}`")));
            }
            Ok(())
        })
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        self.store.with(|conn| {
            let n = conn.execute("DELETE FROM sessions WHERE id = ?1", [id])?;
            if n == 0 {
                return Err(NexusError::NotFound(format!("session `{id}`")));
            }
            Ok(())
        })
    }

    /// Idempotency: has a tool call with this key already completed? Used by
    /// crash recovery to avoid repeating side effects.
    pub fn tool_call_completed(&self, idempotency_key: &str) -> Result<Option<String>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT output_preview FROM tool_calls
                 WHERE idempotency_key = ?1 AND exit_status = 'ok'",
            )?;
            let mut rows = stmt.query([idempotency_key])?;
            Ok(rows.next()?.map(|r| r.get::<_, String>(0)).transpose()?)
        })
    }

    /// Record a completed (or failed) tool call for audit + idempotency.
    #[allow(clippy::too_many_arguments)]
    pub fn record_tool_call(
        &self,
        session_id: &str,
        trace_id: &str,
        call_id: &str,
        tool: &str,
        arguments_redacted: &str,
        risk: &str,
        decision: &str,
        exit_status: &str,
        output_preview: &str,
        idempotency_key: Option<&str>,
        duration_ms: i64,
    ) -> Result<()> {
        self.store.with(|conn| {
            conn.execute(
                "INSERT INTO tool_calls
                 (id, session_id, trace_id, tool, arguments, risk, decision, exit_status,
                  output_preview, idempotency_key, started_at, finished_at, duration_ms)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?11,?12)
                 ON CONFLICT(idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING",
                rusqlite::params![
                    call_id,
                    session_id,
                    trace_id,
                    tool,
                    arguments_redacted,
                    risk,
                    decision,
                    exit_status,
                    output_preview,
                    idempotency_key,
                    nexus_core::now_rfc3339(),
                    duration_ms
                ],
            )?;
            Ok(())
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_usage(
        &self,
        session_id: &str,
        provider: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        tool_calls: u64,
        elapsed_ms: u64,
    ) -> Result<()> {
        let now = nexus_core::now_rfc3339();
        self.store.with(|conn| {
            conn.execute(
                "INSERT INTO session_usage
                 (session_id, provider, model, input_tokens, output_tokens, tool_calls,
                 elapsed_ms, started_at, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8)
                 ON CONFLICT(session_id) DO UPDATE SET
                   provider = CASE WHEN excluded.provider = ''
                              THEN session_usage.provider ELSE excluded.provider END,
                   model = CASE WHEN excluded.model = ''
                           THEN session_usage.model ELSE excluded.model END,
                   input_tokens = session_usage.input_tokens + excluded.input_tokens,
                   output_tokens = session_usage.output_tokens + excluded.output_tokens,
                   tool_calls = session_usage.tool_calls + excluded.tool_calls,
                   elapsed_ms = session_usage.elapsed_ms + excluded.elapsed_ms,
                   updated_at = excluded.updated_at",
                rusqlite::params![
                    session_id,
                    provider,
                    model,
                    input_tokens as i64,
                    output_tokens as i64,
                    tool_calls as i64,
                    elapsed_ms as i64,
                    now,
                ],
            )?;
            Ok(())
        })
    }

    pub fn usage(&self, session_id: &str) -> Result<SessionUsage> {
        self.store.with(|conn| {
            conn.query_row(
                "SELECT session_id, provider, model, input_tokens, output_tokens,
                        tool_calls, elapsed_ms, started_at, updated_at, exit_at
                 FROM session_usage WHERE session_id = ?1",
                [session_id],
                |row| {
                    Ok(SessionUsage {
                        session_id: row.get(0)?,
                        provider: row.get(1)?,
                        model: row.get(2)?,
                        input_tokens: row.get::<_, i64>(3)?.max(0) as u64,
                        output_tokens: row.get::<_, i64>(4)?.max(0) as u64,
                        tool_calls: row.get::<_, i64>(5)?.max(0) as u64,
                        elapsed_ms: row.get::<_, i64>(6)?.max(0) as u64,
                        started_at: row.get(7)?,
                        updated_at: row.get(8)?,
                        exit_at: row.get(9)?,
                    })
                },
            )
            .map_err(|_| NexusError::NotFound(format!("usage for session `{session_id}`")))
        })
    }

    pub fn usage_or_default(&self, session_id: &str) -> Result<SessionUsage> {
        match self.usage(session_id) {
            Ok(usage) => Ok(usage),
            Err(NexusError::NotFound(_)) => Ok(SessionUsage {
                session_id: session_id.to_string(),
                ..Default::default()
            }),
            Err(e) => Err(e),
        }
    }

    pub fn mark_exit(&self, session_id: &str) -> Result<()> {
        let meta = self.get(session_id)?;
        self.record_usage(session_id, "", &meta.model, 0, 0, 0, 0)?;
        self.store.with(|conn| {
            conn.execute(
                "UPDATE session_usage SET exit_at = ?1, updated_at = ?1 WHERE session_id = ?2",
                rusqlite::params![nexus_core::now_rfc3339(), session_id],
            )?;
            Ok(())
        })
    }

    pub fn add_approval_grant(&self, session_id: &str, grant_token: &str) -> Result<()> {
        self.store.with(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO session_approval_grants
                 (session_id, grant_token, created_at) VALUES (?1,?2,?3)",
                rusqlite::params![session_id, grant_token, nexus_core::now_rfc3339()],
            )?;
            Ok(())
        })
    }

    pub fn approval_grants(&self, session_id: &str) -> Result<Vec<String>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT grant_token FROM session_approval_grants
                 WHERE session_id = ?1 ORDER BY created_at, grant_token",
            )?;
            let rows = stmt.query_map([session_id], |row| row.get(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    pub fn add_workspace_approval_grant(&self, workspace: &str, grant_token: &str) -> Result<()> {
        self.store.with(|conn| {
            conn.execute(
                "INSERT INTO workspace_approval_grants
                 (workspace, grant_token, created_at, revoked_at) VALUES (?1,?2,?3,NULL)
                 ON CONFLICT(workspace, grant_token) DO UPDATE SET revoked_at=NULL, created_at=excluded.created_at",
                rusqlite::params![workspace, grant_token, nexus_core::now_rfc3339()],
            )?;
            Ok(())
        })
    }

    pub fn workspace_approval_grants(&self, workspace: &str) -> Result<Vec<String>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT grant_token FROM workspace_approval_grants
                 WHERE workspace=?1 AND revoked_at IS NULL ORDER BY created_at, grant_token",
            )?;
            let rows = stmt.query_map([workspace], |row| row.get(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Into::into)
        })
    }

    pub fn revoke_workspace_approval_grant(
        &self,
        workspace: &str,
        grant_token: &str,
    ) -> Result<bool> {
        self.store.with(|conn| {
            Ok(conn.execute(
                "UPDATE workspace_approval_grants SET revoked_at=?3
                 WHERE workspace=?1 AND grant_token=?2 AND revoked_at IS NULL",
                rusqlite::params![workspace, grant_token, nexus_core::now_rfc3339()],
            )? > 0)
        })
    }

    pub fn link_rollover(&self, parent: &str, child: &str) -> Result<()> {
        self.store.with(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO session_links
                 (parent_session_id, child_session_id, relation, created_at)
                 VALUES (?1,?2,'rollover',?3)",
                rusqlite::params![parent, child, nexus_core::now_rfc3339()],
            )?;
            Ok(())
        })
    }

    pub fn rollover_parent(&self, child: &str) -> Result<Option<String>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT parent_session_id FROM session_links
                 WHERE child_session_id = ?1 AND relation = 'rollover' LIMIT 1",
            )?;
            let mut rows = stmt.query([child])?;
            Ok(rows.next()?.map(|row| row.get(0)).transpose()?)
        })
    }

    /// Stable idempotency scope shared by a session and every continuation
    /// child. This prevents a resumed stage from repeating completed writes
    /// merely because `/continue` created a new session id.
    pub fn rollover_root(&self, session: &str) -> Result<String> {
        let mut current = session.to_string();
        let mut visited = std::collections::BTreeSet::new();
        while visited.insert(current.clone()) {
            let Some(parent) = self.rollover_parent(&current)? else {
                return Ok(current);
            };
            current = parent;
        }
        Err(nexus_core::NexusError::Other(
            "session rollover links contain a cycle".into(),
        ))
    }

    pub fn rollover_children(&self, parent: &str) -> Result<Vec<String>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT child_session_id FROM session_links
                 WHERE parent_session_id = ?1 AND relation = 'rollover'
                 ORDER BY created_at",
            )?;
            let rows = stmt.query_map([parent], |row| row.get(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    pub fn rollover(&self, parent: &str, approved_summary: &str) -> Result<SessionId> {
        let source = self.get(parent)?;
        let child = self.create(&source.workspace, &source.agent, &source.model)?;
        self.rename(child.as_str(), &source.title)?;
        self.set_summary(child.as_str(), approved_summary)?;
        self.set_persona_profile(
            child.as_str(),
            source.persona_id.as_deref(),
            &source.profile_name,
        )?;
        self.add_message(
            child.as_str(),
            0,
            &ChatMessage::user(format!(
                "Approved rollover handoff from session {parent}:\n\n{approved_summary}"
            )),
        )?;
        self.link_rollover(parent, child.as_str())?;
        Ok(child)
    }
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn reconstruct_message(
    role: &str,
    content: &str,
    tool_call_id: Option<String>,
    tool_name: Option<String>,
) -> ChatMessage {
    match role {
        "system" => ChatMessage::system(content),
        "user" => ChatMessage::user(content),
        "tool" => ChatMessage {
            role: Role::Tool,
            content: content.to_string(),
            tool_calls: vec![],
            tool_call_id,
            name: tool_name,
        },
        _ => {
            // Assistant: may carry embedded tool calls.
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(content) {
                if let Some(calls) = v.get("tool_calls").and_then(|c| c.as_array()) {
                    let tool_calls = calls
                        .iter()
                        .filter_map(|c| {
                            Some(nexus_models::types::ToolCallRequest {
                                id: c.get("id")?.as_str()?.to_string(),
                                name: c.get("name")?.as_str()?.to_string(),
                                arguments: c.get("arguments")?.as_str()?.to_string(),
                            })
                        })
                        .collect();
                    return ChatMessage {
                        role: Role::Assistant,
                        content: v
                            .get("text")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string(),
                        tool_calls,
                        tool_call_id: None,
                        name: None,
                    };
                }
            }
            ChatMessage::assistant(content)
        }
    }
}

fn row_to_meta(row: &rusqlite::Row) -> rusqlite::Result<SessionMeta> {
    let parse_vec = |s: String| -> Vec<String> { serde_json::from_str(&s).unwrap_or_default() };
    Ok(SessionMeta {
        id: SessionId::from(row.get::<_, String>(0)?),
        title: row.get(1)?,
        workspace: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        model: row.get(5)?,
        agent: row.get(6)?,
        summary: row.get(7)?,
        pending_tasks: parse_vec(row.get(8)?),
        changed_files: parse_vec(row.get(9)?),
        current_goal: row.get(10)?,
        status: row.get(11)?,
        persona_id: row.get(12)?,
        profile_name: row.get(13)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sessions() -> SessionStore {
        SessionStore::new(Store::open_in_memory().expect("store"))
    }

    #[test]
    fn create_and_reload_messages() {
        let s = sessions();
        let id = s
            .create("/ws", "orchestrator", "local_main")
            .expect("create");
        s.add_message(id.as_str(), 1, &ChatMessage::user("hello"))
            .expect("add");
        let mut assistant = ChatMessage::assistant("working on it");
        assistant
            .tool_calls
            .push(nexus_models::types::ToolCallRequest {
                id: "call_1".into(),
                name: "fs.read_file".into(),
                arguments: "{\"path\":\"a\"}".into(),
            });
        s.add_message(id.as_str(), 1, &assistant).expect("add");
        s.add_message(
            id.as_str(),
            1,
            &ChatMessage::tool_result("call_1", "fs.read_file", "content"),
        )
        .expect("add");

        let messages = s.messages(id.as_str()).expect("messages");
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1].tool_calls.len(), 1);
        assert_eq!(messages[1].tool_calls[0].name, "fs.read_file");
        assert_eq!(messages[2].role, Role::Tool);
    }

    #[test]
    fn idempotency_prevents_repeat() {
        let s = sessions();
        let id = s.create("/ws", "a", "m").expect("create");
        s.record_tool_call(
            id.as_str(),
            "trace_1",
            "call_1",
            "fs.create_file",
            "{}",
            "write",
            "allow",
            "ok",
            "wrote file",
            Some("idem-123"),
            5,
        )
        .expect("record");
        assert_eq!(
            s.tool_call_completed("idem-123").expect("check"),
            Some("wrote file".into())
        );
        assert_eq!(s.tool_call_completed("idem-999").expect("check"), None);
    }

    #[test]
    fn changed_files_deduplicated() {
        let s = sessions();
        let id = s.create("/ws", "a", "m").expect("create");
        s.record_changed_file(id.as_str(), "src/a.rs")
            .expect("record");
        s.record_changed_file(id.as_str(), "src/a.rs")
            .expect("record");
        s.record_changed_file(id.as_str(), "src/b.rs")
            .expect("record");
        assert_eq!(s.get(id.as_str()).expect("get").changed_files.len(), 2);
    }

    #[test]
    fn usage_rollover_and_session_grants_are_durable() {
        let s = sessions();
        let parent = s.create("/ws", "planner", "m").expect("create");
        s.rename(parent.as_str(), "Upgrade plan").expect("rename");
        s.set_persona_profile(parent.as_str(), Some("persona_1"), "focused")
            .expect("persona/profile");
        s.record_usage(parent.as_str(), "mock", "m", 10, 4, 2, 150)
            .expect("usage");
        s.record_usage(parent.as_str(), "mock", "m", 5, 1, 1, 50)
            .expect("usage again");
        s.add_approval_grant(parent.as_str(), "cmd:[\"cargo\",\"check\"]")
            .expect("grant");

        let usage = s.usage(parent.as_str()).expect("usage read");
        assert_eq!((usage.input_tokens, usage.output_tokens), (15, 5));
        assert_eq!((usage.tool_calls, usage.elapsed_ms), (3, 200));
        assert_eq!(
            s.approval_grants(parent.as_str()).expect("grants"),
            vec!["cmd:[\"cargo\",\"check\"]".to_string()]
        );

        let child = s
            .rollover(parent.as_str(), "Approved handoff")
            .expect("rollover");
        let child_meta = s.get(child.as_str()).expect("child");
        assert_eq!(child_meta.title, "Upgrade plan");
        assert_eq!(child_meta.persona_id.as_deref(), Some("persona_1"));
        assert_eq!(child_meta.profile_name, "focused");
        let messages = s.messages(child.as_str()).expect("child messages");
        assert_eq!(messages.len(), 1);
        assert!(messages[0].content.contains("Approved handoff"));
        assert!(s
            .approval_grants(child.as_str())
            .expect("child grants")
            .is_empty());
        assert_eq!(
            s.rollover_parent(child.as_str()).expect("parent"),
            Some(parent.as_str().to_string())
        );
        assert_eq!(
            s.rollover_children(parent.as_str()).expect("children"),
            vec![child.as_str().to_string()]
        );
        let grandchild = s
            .rollover(child.as_str(), "Second approved handoff")
            .expect("grandchild");
        assert_eq!(
            s.rollover_root(grandchild.as_str()).expect("root"),
            parent.as_str()
        );
    }

    #[test]
    fn workspace_grants_are_scoped_and_revocable() {
        let s = sessions();
        s.add_workspace_approval_grant("/ws-a", "cmd:[[\"git\",\"status\"]]")
            .expect("grant");
        assert_eq!(
            s.workspace_approval_grants("/ws-a").expect("list"),
            vec!["cmd:[[\"git\",\"status\"]]".to_string()]
        );
        assert!(s
            .workspace_approval_grants("/ws-b")
            .expect("other workspace")
            .is_empty());
        assert!(s
            .revoke_workspace_approval_grant("/ws-a", "cmd:[[\"git\",\"status\"]]")
            .expect("revoke"));
        assert!(s
            .workspace_approval_grants("/ws-a")
            .expect("list after revoke")
            .is_empty());
    }
}
