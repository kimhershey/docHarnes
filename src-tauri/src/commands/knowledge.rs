//! Knowledge-base commands (P3): collections, versions, glossary, indexing
//! and embedding jobs, hybrid search, grounded answers.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use doculens_core::documents::{repo as docs, DocumentRole};
use doculens_core::knowledge::index::index_status;
use doculens_core::knowledge::search::search;
use doculens_core::knowledge::{
    chat, embed, repo, Answer, AnswerKind, AnswerRequest, Chunk, Collection, CollectionInput, GlossaryEntry,
    GlossaryInput, IndexStatus, SavedAnswer, SearchRequest, SearchResponse, VersionGroup, VersionInfo, VersionInput,
};

use crate::commands::documents::{view, DocumentView};
use crate::docs_bridge;
use crate::knowledge_bridge::{self, EmbeddingModelInfo};
use crate::state::{UiError, UiResult};

/// Repository functions signal expected failures as `rusqlite::Error::InvalidParameterName(code)`
/// (surfacing as `CoreError::Db("Invalid parameter name: <code>")`); map those to typed UI errors.
fn typed_error(e: doculens_core::CoreError) -> UiError {
    const CODES: [&str; 8] = [
        "collection_name_taken",
        "collection_name_empty",
        "collection_not_found",
        "document_not_found",
        "glossary_term_empty",
        "glossary_term_taken",
        "version_group_empty",
        "feedback_invalid",
    ];
    if let doculens_core::CoreError::Db(m) = &e {
        if let Some(code) = CODES.iter().find(|c| m.contains(*c)) {
            return UiError::invalid(*code);
        }
        if m.contains("UNIQUE constraint failed: collections.name") {
            return UiError::invalid("collection_name_taken");
        }
        if m.contains("UNIQUE constraint failed: glossary.term") {
            return UiError::invalid("glossary_term_taken");
        }
    }
    e.into()
}

#[derive(Debug, Clone, Serialize)]
pub struct KnowledgeStatusView {
    #[serde(flatten)]
    pub status: IndexStatus,
    pub embedding: EmbeddingModelInfo,
    pub index_running: bool,
    pub embed_running: bool,
}

#[tauri::command(async)]
pub fn knowledge_status(app: AppHandle) -> UiResult<KnowledgeStatusView> {
    let ctx = knowledge_bridge::context(&app)?;
    let mut status = index_status(&ctx.index_deps())?;
    if status.tokenizer.kind != "ko-dic" {
        // Tell the user where the dictionary belongs instead of leaving the path empty.
        status.tokenizer.path = Some(knowledge_bridge::expected_kodic_dir(&app).display().to_string());
    }
    let state = app.state::<crate::state::AppState>();
    Ok(KnowledgeStatusView {
        status,
        embedding: knowledge_bridge::embedding_model(&app),
        index_running: state.jobs.is_active_kind(&[crate::jobs::JobKind::KnowledgeIndex]),
        embed_running: state.jobs.is_active_kind(&[crate::jobs::JobKind::KnowledgeEmbed]),
    })
}

// -- collections ------------------------------------------------------------

#[tauri::command(async)]
pub fn knowledge_collections(app: AppHandle) -> UiResult<Vec<Collection>> {
    let ctx = knowledge_bridge::context(&app)?;
    Ok(ctx.db.with_read(repo::list_collections)?)
}

#[tauri::command(async)]
pub fn knowledge_collection_save(app: AppHandle, input: CollectionInput) -> UiResult<Collection> {
    let ctx = knowledge_bridge::context(&app)?;
    let saved = ctx.db.with(|c| repo::save_collection(c, &input)).map_err(typed_error)?;
    ctx.emit(&app, "collections", vec![saved.id.clone()]);
    Ok(saved)
}

#[tauri::command(async)]
pub fn knowledge_collection_delete(app: AppHandle, id: String) -> UiResult<bool> {
    let ctx = knowledge_bridge::context(&app)?;
    let ok = ctx.db.with(|c| repo::delete_collection(c, &id))?;
    ctx.emit(&app, "collections", vec![id]);
    Ok(ok)
}

#[tauri::command(async)]
pub fn knowledge_collection_docs(app: AppHandle, id: String) -> UiResult<Vec<DocumentView>> {
    let ctx = knowledge_bridge::context(&app)?;
    let docs = ctx.db.with_read(|c| repo::collection_documents(c, &id))?;
    docs.into_iter().map(|d| view(&app, d)).collect()
}

#[derive(Debug, Clone, Deserialize)]
pub struct CollectionDocsArgs {
    pub id: String,
    pub doc_ids: Vec<String>,
    /// Also mark the documents as knowledge documents (role) — the ingest wizard default.
    #[serde(default)]
    pub set_role: bool,
}

#[tauri::command(async)]
pub fn knowledge_collection_add(app: AppHandle, args: CollectionDocsArgs) -> UiResult<u32> {
    let ctx = knowledge_bridge::context(&app)?;
    let n = ctx
        .db
        .with_tx(|tx| {
            let n = repo::add_docs_to_collection(tx, &args.id, &args.doc_ids)?;
            if args.set_role {
                for d in &args.doc_ids {
                    docs::set_role(tx, d, DocumentRole::Knowledge)?;
                }
            }
            Ok(n)
        })
        .map_err(typed_error)?;
    ctx.emit(&app, "collections", vec![args.id]);
    if args.set_role {
        docs_bridge::emit_changed(&app, "updated", args.doc_ids);
    }
    Ok(n)
}

#[tauri::command(async)]
pub fn knowledge_collection_remove(app: AppHandle, args: CollectionDocsArgs) -> UiResult<u32> {
    let ctx = knowledge_bridge::context(&app)?;
    let n = ctx.db.with(|c| repo::remove_docs_from_collection(c, &args.id, &args.doc_ids))?;
    ctx.emit(&app, "collections", vec![args.id]);
    Ok(n)
}

#[tauri::command(async)]
pub fn knowledge_document_collections(app: AppHandle, id: String) -> UiResult<Vec<(String, String)>> {
    let ctx = knowledge_bridge::context(&app)?;
    Ok(ctx.db.with_read(|c| repo::collections_of_document(c, &id))?)
}

/// Sets the role of documents. The role follows the memberships: a document that is still in a reference
/// group (the common material, another project's group) stays reference material when it is asked to become
/// a work document — the project screen's 참고 자료 → 업무 문서 switch ends with this call, and it must not
/// change the document for everyone else. Returns the number of documents that now have `role`.
#[tauri::command(async)]
pub fn knowledge_set_role(app: AppHandle, ids: Vec<String>, role: DocumentRole) -> UiResult<u32> {
    let ctx = knowledge_bridge::context(&app)?;
    let n = ctx.db.with_tx(|tx| {
        let mut n = 0;
        for d in &ids {
            if repo::set_role_by_membership(tx, d, role)? == role {
                n += 1;
            }
        }
        Ok(n)
    })?;
    docs_bridge::emit_changed(&app, "updated", ids);
    ctx.emit(&app, "status", Vec::new());
    Ok(n)
}

// -- indexing / embeddings --------------------------------------------------

#[tauri::command(async)]
pub fn knowledge_index(app: AppHandle, ids: Option<Vec<String>>, force: Option<bool>) -> UiResult<Option<String>> {
    knowledge_bridge::index_job(&app, ids, force.unwrap_or(false))
}

#[tauri::command(async)]
pub fn knowledge_embed(app: AppHandle, load_model: Option<bool>) -> UiResult<Option<String>> {
    knowledge_bridge::embed_job(&app, load_model.unwrap_or(true))
}

#[tauri::command(async)]
pub fn knowledge_reset_embeddings(app: AppHandle) -> UiResult<()> {
    let ctx = knowledge_bridge::context(&app)?;
    ctx.db.with(embed::reset_embeddings)?;
    ctx.emit(&app, "embedded", Vec::new());
    Ok(())
}

// -- search / answers -------------------------------------------------------

#[tauri::command(async)]
pub fn knowledge_search(app: AppHandle, mut req: SearchRequest) -> UiResult<SearchResponse> {
    req.query = req.query.chars().take(2_000).collect();
    let ctx = knowledge_bridge::context(&app)?;
    let embedder = knowledge_bridge::embedder(&app, false)?;
    let deps = ctx.search_deps(embedder.as_ref().map(|e| e as &dyn embed::Embedder));
    Ok(search(&deps, &req)?)
}

#[tauri::command(async)]
pub fn knowledge_chunks(app: AppHandle, document_id: String) -> UiResult<Vec<Chunk>> {
    let ctx = knowledge_bridge::context(&app)?;
    Ok(ctx.db.with_read(|c| repo::list_chunks(c, &document_id))?)
}

/// Grounded answer; `ui` (what the screen shows) becomes the request's session context — the
/// open document, selection, folder and project by name — unless the request carries one already.
#[tauri::command(async)]
pub fn knowledge_answer(
    app: AppHandle,
    req: AnswerRequest,
    request_id: Option<String>,
    ui: Option<crate::planner_bridge::UiPlanContext>,
) -> UiResult<Answer> {
    let request_id = request_id.unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
    knowledge_bridge::answer(&app, req, request_id, ui)
}

#[tauri::command(async)]
pub fn knowledge_answer_cancel(app: AppHandle, request_id: String) -> UiResult<bool> {
    Ok(knowledge_bridge::cancel_answer(&app, &request_id))
}

#[tauri::command(async)]
pub fn knowledge_answers(
    app: AppHandle,
    kind: Option<AnswerKind>,
    document_id: Option<String>,
    limit: Option<u32>,
) -> UiResult<Vec<SavedAnswer>> {
    let ctx = knowledge_bridge::context(&app)?;
    Ok(ctx.db.with_read(|c| repo::list_answers(c, kind, document_id.as_deref(), limit.unwrap_or(50).min(500)))?)
}

#[tauri::command(async)]
pub fn knowledge_answer_feedback(
    app: AppHandle,
    id: i64,
    feedback: Option<i8>,
    note: Option<String>,
) -> UiResult<bool> {
    if !matches!(feedback, None | Some(1) | Some(-1)) {
        return Err(UiError::invalid("feedback_invalid"));
    }
    let note: Option<String> = note.map(|n| n.chars().take(2_000).collect());
    let ctx = knowledge_bridge::context(&app)?;
    let ok = ctx.db.with(|c| repo::set_answer_feedback(c, id, feedback, note.as_deref()))?;
    ctx.emit(&app, "answers", vec![id.to_string()]);
    Ok(ok)
}

#[tauri::command(async)]
pub fn knowledge_answer_delete(app: AppHandle, id: i64) -> UiResult<bool> {
    let ctx = knowledge_bridge::context(&app)?;
    let ok = ctx.db.with(|c| repo::delete_answer(c, id))?;
    ctx.emit(&app, "answers", vec![id.to_string()]);
    Ok(ok)
}

// -- chat sessions (0.7.0 item 6) -------------------------------------------

#[tauri::command(async)]
pub fn chat_sessions_list(app: AppHandle) -> UiResult<Vec<chat::ChatSessionSummary>> {
    let ctx = knowledge_bridge::context(&app)?;
    Ok(ctx.db.with_read(chat::list_sessions)?)
}

#[tauri::command(async)]
pub fn chat_session_create(app: AppHandle, title: Option<String>) -> UiResult<chat::ChatSessionSummary> {
    let ctx = knowledge_bridge::context(&app)?;
    Ok(ctx.db.with(|c| chat::create_session(c, title.as_deref()))?)
}

#[tauri::command(async)]
pub fn chat_session_rename(app: AppHandle, id: String, title: String) -> UiResult<bool> {
    if title.trim().is_empty() {
        return Err(UiError::invalid("title_empty"));
    }
    let ctx = knowledge_bridge::context(&app)?;
    Ok(ctx.db.with(|c| chat::rename_session(c, &id, &title))?)
}

#[tauri::command(async)]
pub fn chat_session_delete(app: AppHandle, id: String) -> UiResult<bool> {
    let ctx = knowledge_bridge::context(&app)?;
    Ok(ctx.db.with(|c| chat::delete_session(c, &id))?)
}

#[tauri::command(async)]
pub fn chat_messages_list(app: AppHandle, session_id: String) -> UiResult<Vec<chat::ChatMessageRow>> {
    let ctx = knowledge_bridge::context(&app)?;
    Ok(ctx.db.with_read(|c| chat::list_messages(c, &session_id))?)
}

/// Stores or replaces one message of a session (`payload` = the UI's message JSON, kept as is).
#[tauri::command(async)]
pub fn chat_message_put(app: AppHandle, session_id: String, seq: i64, payload: serde_json::Value) -> UiResult<bool> {
    if payload.to_string().len() > 2_000_000 {
        return Err(UiError::invalid("chat_message_too_large"));
    }
    let ctx = knowledge_bridge::context(&app)?;
    Ok(ctx.db.with(|c| chat::put_message(c, &session_id, seq, &payload))?)
}

/// Drops the messages of a session from `from_seq` on.
#[tauri::command(async)]
pub fn chat_messages_truncate(app: AppHandle, session_id: String, from_seq: i64) -> UiResult<u64> {
    let ctx = knowledge_bridge::context(&app)?;
    Ok(ctx.db.with(|c| chat::truncate_messages(c, &session_id, from_seq))? as u64)
}

// -- glossary / versions ----------------------------------------------------

#[tauri::command(async)]
pub fn knowledge_glossary(app: AppHandle) -> UiResult<Vec<GlossaryEntry>> {
    let ctx = knowledge_bridge::context(&app)?;
    Ok(ctx.db.with_read(repo::list_glossary)?)
}

#[tauri::command(async)]
pub fn knowledge_glossary_save(app: AppHandle, input: GlossaryInput) -> UiResult<GlossaryEntry> {
    let ctx = knowledge_bridge::context(&app)?;
    let saved = ctx.db.with(|c| repo::save_glossary(c, &input)).map_err(typed_error)?;
    ctx.emit(&app, "glossary", vec![saved.id.to_string()]);
    Ok(saved)
}

#[tauri::command(async)]
pub fn knowledge_glossary_delete(app: AppHandle, id: i64) -> UiResult<bool> {
    let ctx = knowledge_bridge::context(&app)?;
    let ok = ctx.db.with(|c| repo::delete_glossary(c, id))?;
    ctx.emit(&app, "glossary", vec![id.to_string()]);
    Ok(ok)
}

#[tauri::command(async)]
pub fn knowledge_versions(app: AppHandle) -> UiResult<Vec<VersionGroup>> {
    let ctx = knowledge_bridge::context(&app)?;
    Ok(ctx.db.with_read(repo::list_version_groups)?)
}

#[tauri::command(async)]
pub fn knowledge_version_set(app: AppHandle, input: VersionInput) -> UiResult<VersionInfo> {
    let ctx = knowledge_bridge::context(&app)?;
    let v = ctx.db.with(|c| repo::set_version(c, &input)).map_err(typed_error)?;
    ctx.emit(&app, "versions", vec![v.document_id.clone()]);
    Ok(v)
}

#[tauri::command(async)]
pub fn knowledge_version_clear(app: AppHandle, document_id: String) -> UiResult<bool> {
    let ctx = knowledge_bridge::context(&app)?;
    let ok = ctx.db.with(|c| repo::clear_version(c, &document_id))?;
    ctx.emit(&app, "versions", vec![document_id]);
    Ok(ok)
}
