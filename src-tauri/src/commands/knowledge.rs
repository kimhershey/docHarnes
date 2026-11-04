//! Knowledge-base commands (P3): collections, versions, glossary, indexing
//! and embedding jobs, hybrid search, grounded answers.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use doculens_core::documents::{repo as docs, DocumentRole};
use doculens_core::knowledge::index::index_status;
use doculens_core::knowledge::search::search;
use doculens_core::knowledge::{
    embed, repo, Answer, AnswerKind, AnswerRequest, Chunk, Collection, CollectionInput, GlossaryEntry, GlossaryInput,
    IndexStatus, SavedAnswer, SearchRequest, SearchResponse, VersionGroup, VersionInfo, VersionInput,
};

use crate::commands::documents::{view, DocumentView};
use crate::docs_bridge;
use crate::knowledge_bridge::{self, EmbeddingModelInfo};
use crate::state::{UiError, UiResult};

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
    let status = index_status(&ctx.index_deps())?;
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
    Ok(ctx.db.with(repo::list_collections)?)
}

#[tauri::command(async)]
pub fn knowledge_collection_save(app: AppHandle, input: CollectionInput) -> UiResult<Collection> {
    let ctx = knowledge_bridge::context(&app)?;
    let saved = ctx.db.with(|c| repo::save_collection(c, &input)).map_err(|e| match e {
        doculens_core::CoreError::Db(m) if m.contains("UNIQUE") => UiError::invalid("collection_name_taken"),
        doculens_core::CoreError::Db(m) if m.contains("empty") => UiError::invalid("collection_name_empty"),
        other => other.into(),
    })?;
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
    let ids = ctx.db.with(|c| repo::collection_doc_ids(c, &id))?;
    let mut out = Vec::new();
    for d in ids {
        if let Some(doc) = ctx.db.with(|c| docs::get_document(c, &d))? {
            out.push(view(&app, doc)?);
        }
    }
    Ok(out)
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
    let n = ctx.db.with_tx(|tx| {
        let n = repo::add_docs_to_collection(tx, &args.id, &args.doc_ids)?;
        if args.set_role {
            for d in &args.doc_ids {
                docs::set_role(tx, d, DocumentRole::Knowledge)?;
            }
        }
        Ok(n)
    })?;
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
    Ok(ctx.db.with(|c| repo::collections_of_document(c, &id))?)
}

#[tauri::command(async)]
pub fn knowledge_set_role(app: AppHandle, ids: Vec<String>, role: DocumentRole) -> UiResult<u32> {
    let ctx = knowledge_bridge::context(&app)?;
    let n = ctx.db.with_tx(|tx| {
        let mut n = 0;
        for d in &ids {
            docs::set_role(tx, d, role)?;
            n += 1;
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
pub fn knowledge_search(app: AppHandle, req: SearchRequest) -> UiResult<SearchResponse> {
    let ctx = knowledge_bridge::context(&app)?;
    let embedder = knowledge_bridge::embedder(&app, false)?;
    let deps = ctx.search_deps(embedder.as_ref().map(|e| e as &dyn embed::Embedder));
    Ok(search(&deps, &req)?)
}

#[tauri::command(async)]
pub fn knowledge_chunks(app: AppHandle, document_id: String) -> UiResult<Vec<Chunk>> {
    let ctx = knowledge_bridge::context(&app)?;
    Ok(ctx.db.with(|c| repo::list_chunks(c, &document_id))?)
}

#[tauri::command(async)]
pub fn knowledge_answer(app: AppHandle, req: AnswerRequest, request_id: Option<String>) -> UiResult<Answer> {
    let request_id = request_id.unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
    knowledge_bridge::answer(&app, req, request_id)
}

#[tauri::command(async)]
pub fn knowledge_answers(
    app: AppHandle,
    kind: Option<AnswerKind>,
    document_id: Option<String>,
    limit: Option<u32>,
) -> UiResult<Vec<SavedAnswer>> {
    let ctx = knowledge_bridge::context(&app)?;
    Ok(ctx.db.with(|c| repo::list_answers(c, kind, document_id.as_deref(), limit.unwrap_or(50)))?)
}

#[tauri::command(async)]
pub fn knowledge_answer_feedback(
    app: AppHandle,
    id: i64,
    feedback: Option<i8>,
    note: Option<String>,
) -> UiResult<bool> {
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

// -- glossary / versions ----------------------------------------------------

#[tauri::command(async)]
pub fn knowledge_glossary(app: AppHandle) -> UiResult<Vec<GlossaryEntry>> {
    let ctx = knowledge_bridge::context(&app)?;
    Ok(ctx.db.with(repo::list_glossary)?)
}

#[tauri::command(async)]
pub fn knowledge_glossary_save(app: AppHandle, input: GlossaryInput) -> UiResult<GlossaryEntry> {
    let ctx = knowledge_bridge::context(&app)?;
    let saved = ctx.db.with(|c| repo::save_glossary(c, &input)).map_err(|e| match e {
        doculens_core::CoreError::Db(m) if m.contains("empty") => UiError::invalid("glossary_term_empty"),
        other => other.into(),
    })?;
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
    Ok(ctx.db.with(repo::list_version_groups)?)
}

#[tauri::command(async)]
pub fn knowledge_version_set(app: AppHandle, input: VersionInput) -> UiResult<VersionInfo> {
    let ctx = knowledge_bridge::context(&app)?;
    let v = ctx.db.with(|c| repo::set_version(c, &input)).map_err(|e| match e {
        doculens_core::CoreError::Db(m) if m.contains("empty") => UiError::invalid("version_group_empty"),
        other => other.into(),
    })?;
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
