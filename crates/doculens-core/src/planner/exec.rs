//! Deterministic plan executor (§3.5 "결정적 실행기"): replays a validated
//! plan step by step over the documents in scope, reusing the pipeline,
//! knowledge, organise, aggregate and export modules. UI-side ops (settings,
//! navigation, job / engine control) are returned as `ui_actions`.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::aggregate::{self, AggregateRequest};
use crate::db::Db;
use crate::documents::import::{self, ImportDeps, ImportOptions};
use crate::documents::{repo as docs, DocumentRecord, DocumentRole};
use crate::error::{CoreError, Result};
use crate::export::{self, ExportRequest};
use crate::generate::{self, DraftRequest, GenerateDeps, ReviseScope, SourceRef};
use crate::josa;
use crate::knowledge::answer::{self, AnswerDeps, ChatBackend};
use crate::knowledge::embed::Embedder;
use crate::knowledge::repo as knowledge_repo;
use crate::knowledge::search::{self, SearchDeps};
use crate::knowledge::tokenize::Analyzer;
use crate::knowledge::{AnswerKind, AnswerRequest, SearchRequest, SearchScope};
use crate::knowledge::{Collection, CollectionInput, GlossaryInput, VersionInput};
use crate::organize::{self, journal, OrganizeRequest};
use crate::paths::WorkspacePaths;
use crate::pipeline::repo::{self as pipeline_repo, ReviewFilter};
use crate::pipeline::review::{self, ReviewAction};
use crate::pipeline::run::{self as pipeline_run, PipelineDeps};
use crate::pipeline::schema::SchemaDraft;
use crate::pipeline::split;
use crate::pipeline::vlm::Vlm;
use crate::pipeline::{PipelineLevel, ProcessRequest, ReviewReason, SchemaChoice};
use crate::projects::classify::{self, Assignment, ClassifyRequest};
use crate::projects::{self, repo as projects_repo, ProjectDeps, ProjectInput, TemplateInput};
use crate::settings::{KnowledgeSettings, Language, ProcessingSettings, Profile};

use super::{
    compact, scope, AggregateFn, ExplainScope, ExportLayout, ExportTarget, GroupBy, OcrProfile, Op, OpKind, Plan,
    ReferenceAction, ReviewOnly, ReviewOp, SchemaAction, Scope, TemplateAction, TermAction,
};

mod summarize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Running,
    Done,
    Skipped,
    Failed,
    /// Handed to the UI / shell (settings, navigation, job control).
    Ui,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepResult {
    pub index: usize,
    pub op: OpKind,
    pub status: StepStatus,
    /// Short human-readable outcome (Korean / English per context language).
    pub summary: String,
    /// Structured payload (aggregate result, export path, search hits …).
    #[serde(default)]
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PlanExecution {
    pub steps: Vec<StepResult>,
    pub cancelled: bool,
    /// Documents touched by any step (the shell emits `documents://changed`).
    pub documents: Vec<String>,
    /// Ops the UI has to perform (in order).
    pub ui_actions: Vec<Op>,
    /// Journal batch ids created (undo entry points).
    pub batches: Vec<String>,
    /// Files restored from the workspace trash by an undo (the shell re-imports them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restored_paths: Vec<String>,
}

impl PlanExecution {
    pub fn failed(&self) -> bool {
        self.steps.iter().any(|s| s.status == StepStatus::Failed)
    }
}

/// Pre-computed preview of a plan's side effects (confirmation card).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PlanPreview {
    /// Documents in scope.
    pub document_count: u64,
    pub document_ids: Vec<String>,
    /// Per-step previews keyed by op index (organise diff, export path, extract fields …).
    pub steps: Vec<StepPreview>,
    /// Whether the plan needs the card (side effects, overwrite, deletion …).
    pub requires_confirmation: bool,
    /// Whether any step works on the documents in scope (`validate::uses_scope`). `false` for
    /// plans of settings / navigation / administration ops: `document_count` then says nothing
    /// about the plan and the card should not show it.
    #[serde(default)]
    pub uses_scope: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepPreview {
    pub index: usize,
    pub op: OpKind,
    pub summary: String,
    #[serde(default)]
    pub payload: Value,
    /// This step needs explicit confirmation (file moves, overwrite, deletion …).
    pub confirm: bool,
}

pub fn plan_kinds(plan: &Plan) -> Vec<OpKind> {
    plan.operations.iter().map(Op::kind).collect()
}

/// Engine occupancy hook (§3.9 single slot): `begin` before a model-bound step, `end` after it,
/// so interactive requests only wait during model steps, not during imports / exports.
pub trait EngineLease {
    fn begin(&self);
    fn end(&self);
}

/// Releases the lease on every exit path (errors, early returns, unwinding).
struct LeaseGuard<'a>(Option<&'a dyn EngineLease>);

impl<'a> LeaseGuard<'a> {
    fn acquire(lease: Option<&'a dyn EngineLease>) -> LeaseGuard<'a> {
        if let Some(l) = lease {
            l.begin();
        }
        LeaseGuard(lease)
    }
}

impl Drop for LeaseGuard<'_> {
    fn drop(&mut self) {
        if let Some(l) = self.0 {
            l.end();
        }
    }
}

/// Everything the executor needs; the shell assembles it per job.
pub struct ExecDeps<'a> {
    pub import: ImportDeps<'a>,
    /// Chat / vision backend (`None` = ops that need the model fail with `no_model`).
    pub chat: Option<&'a dyn ChatBackend>,
    pub model: Option<String>,
    pub ctx_size: Option<u32>,
    pub settings: ProcessingSettings,
    pub knowledge: KnowledgeSettings,
    pub profile: Profile,
    pub pii_masking: bool,
    pub analyzer: &'a Analyzer,
    pub embedder: Option<&'a dyn Embedder>,
    pub language: Language,
    pub today: NaiveDate,
    pub cancel: Option<&'a AtomicBool>,
    /// Called between documents / pages: pause, engine gate (`Err` = cancelled).
    pub checkpoint: Option<&'a dyn Fn() -> Result<()>>,
    /// Engine occupancy around model-bound steps.
    pub lease: Option<&'a dyn EngineLease>,
}

impl ExecDeps<'_> {
    fn db(&self) -> &Db {
        self.import.db
    }
    fn paths(&self) -> &WorkspacePaths {
        self.import.paths
    }
    fn cancelled(&self) -> bool {
        self.cancel.is_some_and(|c| c.load(Ordering::Relaxed))
    }
    fn checkpoint(&self) -> Result<()> {
        if self.cancelled() {
            return Err(CoreError::Cancelled);
        }
        match self.checkpoint {
            Some(f) => f(),
            None => Ok(()),
        }
    }
    fn ko(&self) -> bool {
        self.language == Language::Ko
    }
    fn t<'b>(&self, ko: &'b str, en: &'b str) -> &'b str {
        if self.ko() {
            ko
        } else {
            en
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ExecProgress {
    pub step: usize,
    pub steps: usize,
    pub op: Option<OpKind>,
    pub stage: String,
    pub done: u32,
    pub total: u32,
    pub current: String,
}

/// Documents the plan works on right now: the scope — or, once an import step has run, the
/// documents of that import (`imported`; see [`batch_scope`]).
fn working_set(deps: &ExecDeps<'_>, plan: &Plan, imported: Option<&[String]>) -> Result<Vec<DocumentRecord>> {
    working_set_in(deps, &plan_scope(plan), imported, false)
}

/// The working set of a classify / OCR / extract step. After an import the scope's type filter
/// cannot apply to a document the classifier has not seen yet ("inbox의 견적서를 전부 처리하고 …"
/// dropped every file it had just imported — audit PLN-4), so untyped documents stay in; a
/// document that has a type must match it.
fn pipeline_set(deps: &ExecDeps<'_>, plan: &Plan, imported: Option<&[String]>) -> Result<Vec<DocumentRecord>> {
    working_set_in(deps, &plan_scope(plan), imported, true)
}

fn working_set_in(
    deps: &ExecDeps<'_>,
    scope: &Scope,
    imported: Option<&[String]>,
    untyped_pass: bool,
) -> Result<Vec<DocumentRecord>> {
    let Some(ids) = imported else {
        return deps.db().with(|c| scope::resolve(c, scope));
    };
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut batch = batch_scope(scope, ids);
    let wanted = if untyped_pass { batch.filter.doc_type.take() } else { None };
    let mut docs = deps.db().with(|c| scope::resolve(c, &batch))?;
    if let Some(t) = wanted {
        docs.retain(|d| d.doc_type.as_deref().is_none_or(|x| x == t));
    }
    Ok(docs)
}

/// The scope of the steps after an import: the documents of that import (new, re-linked and the
/// ones it met again), filtered by what the plan's scope says about content (type, dates, vendor,
/// tags …). Where the documents were before — a folder, a selection, a project — no longer
/// applies. The scope used to be united with the import, so "D:\스캔\a.pdf 가져와서 요약해줘"
/// summarised the whole library (audit PLN-4).
fn batch_scope(scope: &Scope, ids: &[String]) -> Scope {
    let mut s = scope.clone();
    s.folder = None;
    s.doc_ids = ids.to_vec();
    s.filter.project_id = None;
    s
}

/// The scope a step hands to the aggregate / export / organise modules: the plan's own — or,
/// after an import step, that import's documents. `None`: the import brought no documents.
fn step_scope(plan: &Plan, imported: Option<&[String]>) -> Option<Scope> {
    match imported {
        None => Some(plan.scope.clone()),
        Some([]) => None,
        Some(ids) => Some(batch_scope(&plan.scope, ids)),
    }
}

/// Whether the plan's scope restricts the documents by more than a project (or an import has
/// defined the working set): when such a scope matches nothing, a search / answer must find
/// nothing instead of falling back to every document (audit PLN-19).
fn scope_narrows(plan: &Plan, imported: Option<&[String]>) -> bool {
    let mut s = plan.scope.clone();
    s.filter.project_id = None;
    imported.is_some() || !s.is_empty()
}

fn nothing_in_scope(deps: &ExecDeps<'_>) -> StepOutcome {
    StepOutcome {
        summary: deps.t("대상 문서 없음", "No documents in scope").into(),
        payload: json!({ "documents": [] }),
        imported: None,
    }
}

/// The scope a plan's documents come from. "선택한 문서를 한빛 프로젝트에 추가해" names the project as
/// the destination of `add_to_project`, not as a membership filter: documents outside it are the
/// point, so the target project is dropped from the filter for such plans.
pub fn plan_scope(plan: &Plan) -> Scope {
    let mut s = plan.scope.clone();
    let target = s.filter.project_id.as_deref();
    let adds_to_target = plan.operations.iter().any(|op| match op {
        Op::AddToProject { project, .. } => project.is_none() || project.as_deref() == target,
        _ => false,
    });
    if target.is_some() && adds_to_target {
        s.filter.project_id = None;
    }
    s
}

fn ids(docs: &[DocumentRecord]) -> Vec<String> {
    docs.iter().map(|d| d.id.clone()).collect()
}

fn count_label(deps: &ExecDeps<'_>, n: usize) -> String {
    if deps.ko() {
        format!("문서 {n}건")
    } else {
        format!("{n} document{}", if n == 1 { "" } else { "s" })
    }
}

/// Runs the plan. Steps run in order; the first failure stops the plan (later
/// steps are `Skipped`), cancellation marks the execution cancelled.
pub fn run(deps: &ExecDeps<'_>, plan: &Plan, progress: &mut dyn FnMut(&ExecProgress)) -> Result<PlanExecution> {
    let mut out = PlanExecution::default();
    let steps = plan.operations.len();
    let mut imported: Option<Vec<String>> = None;
    let mut stop = false;
    for (index, op) in plan.operations.iter().enumerate() {
        let kind = op.kind();
        if stop || deps.cancelled() {
            out.steps.push(StepResult {
                index,
                op: kind,
                status: if deps.cancelled() { StepStatus::Cancelled } else { StepStatus::Skipped },
                summary: String::new(),
                payload: Value::Null,
                error: None,
                elapsed_ms: 0,
            });
            if deps.cancelled() {
                out.cancelled = true;
            }
            continue;
        }
        if kind.runs_in_ui() {
            out.ui_actions.push(op.clone());
            out.steps.push(StepResult {
                index,
                op: kind,
                status: if matches!(op, Op::Clarify { .. } | Op::Noop {}) {
                    StepStatus::Skipped
                } else {
                    StepStatus::Ui
                },
                summary: String::new(),
                payload: Value::Null,
                error: None,
                elapsed_ms: 0,
            });
            continue;
        }
        let started = Instant::now();
        progress(&ExecProgress {
            step: index,
            steps,
            op: Some(kind),
            stage: kind.as_str().into(),
            ..ExecProgress::default()
        });
        let mut report_progress = |stage: &str, done: u32, total: u32, current: &str| {
            progress(&ExecProgress {
                step: index,
                steps,
                op: Some(kind),
                stage: stage.into(),
                done,
                total,
                current: current.into(),
            });
        };
        let result = run_step(deps, plan, op, imported.as_deref(), &mut report_progress, &mut out);
        let elapsed_ms = started.elapsed().as_millis() as u64;
        match result {
            Ok(StepOutcome { summary, payload, imported: new_imported }) => {
                if let Some(ids) = new_imported {
                    imported = Some(ids);
                }
                out.steps.push(StepResult {
                    index,
                    op: kind,
                    status: StepStatus::Done,
                    summary,
                    payload,
                    error: None,
                    elapsed_ms,
                });
            }
            Err(CoreError::Cancelled) => {
                out.cancelled = true;
                out.steps.push(StepResult {
                    index,
                    op: kind,
                    status: StepStatus::Cancelled,
                    summary: String::new(),
                    payload: Value::Null,
                    error: None,
                    elapsed_ms,
                });
                stop = true;
            }
            Err(e) => {
                out.steps.push(StepResult {
                    index,
                    op: kind,
                    status: StepStatus::Failed,
                    summary: String::new(),
                    payload: Value::Null,
                    error: Some(e.to_string()),
                    elapsed_ms,
                });
                stop = true;
            }
        }
    }
    out.documents.sort();
    out.documents.dedup();
    Ok(out)
}

struct StepOutcome {
    summary: String,
    payload: Value,
    imported: Option<Vec<String>>,
}

fn no_model(deps: &ExecDeps<'_>) -> CoreError {
    CoreError::Invalid(deps.t("no_model", "no_model").into())
}

fn run_step(
    deps: &ExecDeps<'_>,
    plan: &Plan,
    op: &Op,
    imported: Option<&[String]>,
    progress: &mut dyn FnMut(&str, u32, u32, &str),
    out: &mut PlanExecution,
) -> Result<StepOutcome> {
    let db = deps.db();
    match op {
        Op::Import { source, path, paths, recursive, auto_pipeline: _ } => {
            let mut options =
                ImportOptions { destination: import::ImportDestination::Inbox, ..ImportOptions::default() };
            if deps.settings.render_dpi > 0 {
                options.dpi = Some(deps.settings.render_dpi);
            }
            let plan_ = match source {
                super::ImportFrom::Inbox => import::inbox_plan(db, deps.paths(), &options)?,
                super::ImportFrom::Folder => {
                    let p = path.as_deref().ok_or_else(|| CoreError::Invalid("path_required".into()))?;
                    options.destination = import::ImportDestination::Documents;
                    let root = std::path::PathBuf::from(p);
                    let root = if root.is_absolute() { root } else { deps.paths().root.join(root) };
                    let (files, _truncated) = import::collect_paths(std::slice::from_ref(&root), 10_000);
                    // Model-produced plans never recurse into an external folder unless asked to.
                    let files = if recursive.unwrap_or(false) {
                        files
                    } else {
                        files.into_iter().filter(|f| f.parent() == Some(root.as_path())).collect()
                    };
                    import::plan(db, deps.paths(), &files, &options, import::ImportSource::Folder)?
                }
                super::ImportFrom::Files => {
                    // 0.6.0: files the user typed or dropped (absolute paths; relative ones are workspace-relative).
                    if paths.is_empty() {
                        return Err(CoreError::Invalid("paths_required".into()));
                    }
                    options.destination = import::ImportDestination::Documents;
                    let files: Vec<std::path::PathBuf> = paths
                        .iter()
                        .map(|p| {
                            let pb = std::path::PathBuf::from(p.trim());
                            if pb.is_absolute() {
                                pb
                            } else {
                                deps.paths().root.join(pb)
                            }
                        })
                        .collect();
                    import::plan(db, deps.paths(), &files, &options, import::ImportSource::Drop)?
                }
            };
            let total = plan_.importable().count();
            let mut touched = Vec::new();
            let report = import::run(
                &deps.import,
                &plan_,
                deps.cancel,
                &mut |p| progress("import", p.done, p.total, &p.current),
                &mut |id, _ok| touched.push(id.to_string()),
            )?;
            out.documents.extend(report.imported.iter().cloned());
            out.documents.extend(report.relinked.iter().cloned());
            // The later steps work on the documents of this import: the new and re-linked ones and
            // the ones the import met again (a file that is already registered, or whose content
            // is — "inbox 처리" still covers what the watcher registered, and importing a file a
            // second time still summarises that file). Never the whole scope (audit PLN-4).
            let mut all = report.imported.clone();
            all.extend(report.relinked.iter().cloned());
            let existing: Vec<String> = plan_
                .items
                .iter()
                .filter_map(|item| match &item.status {
                    import::ImportItemStatus::AlreadyRegistered { id } => Some(id.clone()),
                    import::ImportItemStatus::Duplicate { of, .. } => Some(of.clone()),
                    _ => None,
                })
                .collect();
            all.extend(existing.iter().cloned());
            all.sort();
            all.dedup();
            if report.cancelled {
                return Err(CoreError::Cancelled);
            }
            let summary = if deps.ko() {
                format!(
                    "가져오기 {}건(신규 {}, 다시 연결 {}, 건너뜀 {}, 실패 {})",
                    total,
                    report.imported.len(),
                    report.relinked.len(),
                    report.skipped,
                    report.failed.len()
                )
            } else {
                format!(
                    "Imported {} (new {}, relinked {}, skipped {}, failed {})",
                    total,
                    report.imported.len(),
                    report.relinked.len(),
                    report.skipped,
                    report.failed.len()
                )
            };
            Ok(StepOutcome {
                summary,
                payload: json!({ "imported": report.imported, "relinked": report.relinked, "existing": existing, "failed": report.failed, "skipped": report.skipped }),
                imported: Some(all),
            })
        }
        Op::Classify { doc_types: _, force } => pipeline_step(
            deps,
            pipeline_set(deps, plan, imported)?,
            PipelineLevel::Classify,
            SchemaChoice::Auto,
            force.unwrap_or(false),
            None,
            Vec::new(),
            progress,
            out,
        ),
        Op::Ocr { pages, profile, force } => pipeline_step(
            deps,
            pipeline_set(deps, plan, imported)?,
            PipelineLevel::Ocr,
            SchemaChoice::Auto,
            force.unwrap_or(false),
            profile.map(|p| match p {
                OcrProfile::Standard => deps.profile,
                OcrProfile::FinePrint => Profile::FinePrint,
            }),
            pages.clone(),
            progress,
            out,
        ),
        Op::Extract { schema_ref, force, .. } => {
            let choice = match schema_ref.as_deref() {
                Some(id) if !id.eq_ignore_ascii_case("auto") => {
                    let found = db.with(|c| pipeline_repo::get_schema(c, id))?;
                    match found {
                        Some(s) => SchemaChoice::Id { id: s.id },
                        None => {
                            let by_type = db.with(|c| pipeline_repo::schema_for_doc_type(c, id))?;
                            match by_type {
                                Some(s) => SchemaChoice::Id { id: s.id },
                                None => return Err(CoreError::Invalid(format!("schema_not_found: {id}"))),
                            }
                        }
                    }
                }
                _ => SchemaChoice::Auto,
            };
            // A document that already has an extraction keeps it — and with it every approval and
            // correction made in review — unless the command asks for a re-run (`force`). Every
            // document in scope used to be extracted again (audit PLN-3).
            let force = force.unwrap_or(false);
            let (todo, kept): (Vec<DocumentRecord>, Vec<DocumentRecord>) =
                pipeline_set(deps, plan, imported)?.into_iter().partition(|d| force || d.last_run_id.is_none());
            if kept.is_empty() {
                return pipeline_step(
                    deps,
                    todo,
                    PipelineLevel::Extract,
                    choice,
                    force,
                    None,
                    Vec::new(),
                    progress,
                    out,
                );
            }
            let note = if deps.ko() {
                format!("이미 추출된 {}건은 건너뜀(다시 추출하려면 '다시 추출')", kept.len())
            } else {
                format!("{} already extracted and skipped (say 're-extract' to run them again)", kept.len())
            };
            let mut outcome = if todo.is_empty() {
                StepOutcome {
                    summary: note,
                    payload: json!({ "documents": [], "processed": [], "extracted": 0, "review_pending": 0 }),
                    imported: None,
                }
            } else {
                let mut o =
                    pipeline_step(deps, todo, PipelineLevel::Extract, choice, false, None, Vec::new(), progress, out)?;
                o.summary = format!("{} · {note}", o.summary);
                o
            };
            if let Some(obj) = outcome.payload.as_object_mut() {
                obj.insert("already_extracted".into(), json!(ids(&kept)));
            }
            Ok(outcome)
        }
        Op::SetDocType { doc_type } => {
            let docs_ = working_set(deps, plan, imported)?;
            for d in &docs_ {
                db.with(|c| docs::set_doc_type(c, &d.id, Some(doc_type), Some("manual")))?;
                out.documents.push(d.id.clone());
            }
            Ok(StepOutcome {
                summary: format!("{} → {doc_type}", count_label(deps, docs_.len())),
                payload: json!({ "documents": ids(&docs_) }),
                imported: None,
            })
        }
        Op::Validate {} => {
            let docs_ = working_set(deps, plan, imported)?;
            let mut flagged = 0usize;
            for (i, d) in docs_.iter().enumerate() {
                deps.checkpoint()?;
                progress("validate", i as u32, docs_.len() as u32, &d.original_name);
                let fields = review::revalidate(
                    db,
                    &d.id,
                    f64::from(deps.settings.review_threshold),
                    deps.settings.audit_sample_percent,
                )?;
                flagged += fields.iter().filter(|f| f.needs_review()).count();
                out.documents.push(d.id.clone());
            }
            Ok(StepOutcome {
                summary: if deps.ko() {
                    format!("{} 검증 · 검토 대기 필드 {flagged}개", count_label(deps, docs_.len()))
                } else {
                    format!("Validated {} · {flagged} fields need review", count_label(deps, docs_.len()))
                },
                payload: json!({ "documents": ids(&docs_), "review_fields": flagged }),
                imported: None,
            })
        }
        Op::Review { action, only, field, value } => {
            let reason = match only {
                Some(ReviewOnly::LowConfidence) => Some(ReviewReason::LowConfidence),
                Some(ReviewOnly::Failed) => Some(ReviewReason::ValidationFailed),
                _ => None,
            };
            // `show`: the review screen is the app's — hand it over as a navigation action
            // (the step used to do nothing and report "0 fields rejected", audit PLN-8).
            if *action == ReviewOp::Show {
                let scoped = !plan.scope.is_empty() || imported.is_some();
                let in_scope: Option<HashSet<String>> =
                    if scoped { Some(ids(&working_set(deps, plan, imported)?).into_iter().collect()) } else { None };
                let pending =
                    db.with(|c| pipeline_repo::review_queue(c, &ReviewFilter { reason, ..ReviewFilter::default() }))?;
                let pending: Vec<&crate::pipeline::FieldRecord> =
                    pending.iter().filter(|f| in_scope.as_ref().is_none_or(|s| s.contains(&f.document_id))).collect();
                let mut documents: Vec<String> = pending.iter().map(|f| f.document_id.clone()).collect();
                documents.sort();
                documents.dedup();
                out.ui_actions.push(Op::Open { target: super::OpenTarget::Screen, reference: "review".into() });
                return Ok(StepOutcome {
                    summary: if deps.ko() {
                        format!("검토 대기 필드 {}개(문서 {}건) — 검토함을 엽니다", pending.len(), documents.len())
                    } else {
                        format!(
                            "{} fields in {} documents wait for review — opening the review queue",
                            pending.len(),
                            documents.len()
                        )
                    },
                    payload: json!({ "action": action, "review_fields": pending.len(), "documents": documents }),
                    imported: None,
                });
            }
            let docs_ = working_set(deps, plan, imported)?;
            let mut changed = 0usize;
            // `correct`: documents whose latest extraction has no such field.
            let mut without_field: Vec<String> = Vec::new();
            for d in &docs_ {
                deps.checkpoint()?;
                match action {
                    ReviewOp::Approve => changed += review::approve_document(db, &d.id, reason)?,
                    ReviewOp::Reject => {
                        let pending = db.with(|c| {
                            pipeline_repo::review_queue(
                                c,
                                &ReviewFilter { document_id: Some(d.id.clone()), reason, ..ReviewFilter::default() },
                            )
                        })?;
                        let fids: Vec<i64> = pending.iter().map(|f| f.id).collect();
                        if !fids.is_empty() {
                            changed += review::apply(db, &fids, &ReviewAction::Reject, Some("planner"))?.len();
                        }
                    }
                    ReviewOp::Correct => {
                        let (Some(field), Some(value)) = (field.as_deref(), value.as_deref()) else {
                            return Err(CoreError::Invalid("correct_needs_field".into()));
                        };
                        let fields = db.with(|c| pipeline_repo::latest_fields(c, &d.id))?;
                        match review_field(&fields, field) {
                            Some(f) => {
                                let action = ReviewAction::Correct { value: value.to_string() };
                                changed += review::apply(db, &[f.id], &action, Some("planner"))?.len();
                            }
                            None => without_field.push(d.id.clone()),
                        }
                    }
                    ReviewOp::Show => {}
                }
                out.documents.push(d.id.clone());
            }
            if *action == ReviewOp::Correct && changed == 0 {
                // Nothing was corrected: say why instead of reporting a finished step.
                return Err(CoreError::Invalid(if docs_.is_empty() {
                    "no_documents".into()
                } else {
                    format!("field_not_found: {}", field.as_deref().unwrap_or_default())
                }));
            }
            let (done_ko, done_en) = match action {
                ReviewOp::Approve => ("승인", "approved"),
                ReviewOp::Reject => ("거부", "rejected"),
                _ => ("수정", "corrected"),
            };
            let mut summary = if deps.ko() {
                format!("필드 {changed}개 {done_ko}")
            } else {
                format!("{changed} fields {done_en}")
            };
            if !without_field.is_empty() {
                summary.push_str(&if deps.ko() {
                    format!(" · 해당 필드가 없는 문서 {}건", without_field.len())
                } else {
                    format!(" · {} documents without that field", without_field.len())
                });
            }
            Ok(StepOutcome {
                summary,
                payload: json!({ "action": action, "changed": changed, "documents": ids(&docs_), "without_field": without_field }),
                imported: None,
            })
        }
        Op::Search { query, mode, top_k } => {
            let narrowed = scope_narrows(plan, imported);
            let docs_ = if plan.scope.is_empty() && imported.is_none() {
                Vec::new()
            } else {
                working_set(deps, plan, imported)?
            };
            if narrowed && docs_.is_empty() {
                // An empty document list means "everywhere" to the search: a scope that matches
                // nothing must find nothing (audit PLN-19).
                return Ok(StepOutcome {
                    summary: if deps.ko() {
                        format!("'{query}' 검색 결과 0건(범위에 해당하는 문서 없음)")
                    } else {
                        format!("0 hits for '{query}' (no document matches the scope)")
                    },
                    payload: serde_json::to_value(crate::knowledge::SearchResponse::default()).unwrap_or(Value::Null),
                    imported: None,
                });
            }
            let mut req = SearchRequest::new(query);
            if let Some(m) = mode {
                req.mode = *m;
            }
            req.limit = top_k.unwrap_or(20).clamp(1, 200);
            // A project scope also sees the project's reference material (0.5.0 plan A).
            req.scope = SearchScope {
                documents: ids(&docs_),
                project_id: plan.scope.filter.project_id.clone(),
                latest_only: deps.knowledge.latest_only,
                ..SearchScope::default()
            };
            let resp = search::search(&SearchDeps { db, analyzer: deps.analyzer, embedder: deps.embedder }, &req)?;
            let n = resp.hits.len();
            Ok(StepOutcome {
                summary: if deps.ko() {
                    format!("'{query}' 검색 결과 {n}건")
                } else {
                    format!("{n} hits for '{query}'")
                },
                payload: serde_json::to_value(&resp).unwrap_or(Value::Null),
                imported: None,
            })
        }
        Op::Answer { question, cite: _, knowledge, mode, reply } => {
            // A ready reply (a greeting, "this cannot be done"): shown as it is — no model, no lookup.
            if let Some(text) = reply.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                let answer = crate::knowledge::Answer {
                    kind: AnswerKind::Answer,
                    question: question.clone(),
                    text: text.to_string(),
                    sentences: vec![crate::knowledge::AnswerSentence { text: text.to_string(), citations: Vec::new() }],
                    ..Default::default()
                };
                return Ok(StepOutcome {
                    summary: crate::pipeline::prompts::truncate(text, 200),
                    payload: serde_json::to_value(&answer).unwrap_or(Value::Null),
                    imported: None,
                });
            }
            let chat = deps.chat.ok_or_else(|| no_model(deps))?;
            let style = match mode {
                Some(crate::planner::AnswerMode::Chat) => crate::knowledge::AnswerStyle::Chat,
                Some(crate::planner::AnswerMode::General) => crate::knowledge::AnswerStyle::General,
                _ => crate::knowledge::AnswerStyle::Grounded,
            };
            // Small talk and a sentence that carries its own text look nothing up.
            let retrieve = knowledge.unwrap_or(true) && style != crate::knowledge::AnswerStyle::Chat;
            let docs_ = if !retrieve || (plan.scope.is_empty() && imported.is_none()) {
                Vec::new()
            } else {
                working_set(deps, plan, imported)?
            };
            if retrieve && docs_.is_empty() && scope_narrows(plan, imported) {
                return Ok(no_documents_answer(deps, question, AnswerKind::Answer));
            }
            let _lease = LeaseGuard::acquire(deps.lease);
            let mut req = AnswerRequest::question(question);
            req.retrieve = retrieve;
            req.style = style;
            // A reply inside a plan run is a step result: the history keeps the answers asked as questions.
            req.save = style != crate::knowledge::AnswerStyle::Chat;
            req.scope = SearchScope {
                documents: ids(&docs_),
                project_id: plan.scope.filter.project_id.clone(),
                latest_only: deps.knowledge.latest_only,
                ..SearchScope::default()
            };
            req.allow_general = deps.knowledge.allow_general_knowledge;
            req.top_k = deps.knowledge.top_k;
            req.context_tokens = deps.knowledge.context_tokens;
            req.min_similarity = Some(deps.knowledge.min_vector_similarity);
            let cancelled = || deps.cancelled();
            let adeps = AnswerDeps {
                search: SearchDeps { db, analyzer: deps.analyzer, embedder: deps.embedder },
                chat,
                model: deps.model.clone(),
                cancelled: Some(&cancelled),
                context_size: deps.ctx_size,
            };
            let ans = answer::answer(&adeps, &req, &mut |_| !deps.cancelled())?;
            Ok(StepOutcome {
                // An answer given from general knowledge has a text of its own to show.
                summary: if ans.no_evidence && !ans.used_general_knowledge {
                    deps.t("근거를 찾지 못했습니다", "No evidence found").into()
                } else {
                    crate::pipeline::prompts::truncate(&ans.text, 200)
                },
                payload: serde_json::to_value(&ans).unwrap_or(Value::Null),
                imported: None,
            })
        }
        Op::Summarize { target, length, group_by } => {
            summarize::run(deps, plan, imported, *target, *length, *group_by, progress)
        }
        Op::Aggregate { func, field, group_by, include_unreviewed } => {
            let Some(step_scope) = step_scope(plan, imported) else { return Ok(nothing_in_scope(deps)) };
            let req = AggregateRequest {
                func: *func,
                field: field.clone(),
                group_by: group_by.unwrap_or(GroupBy::None),
                include_unreviewed: include_unreviewed.unwrap_or(false),
                scope: step_scope,
            };
            let res = aggregate::aggregate(db, &req)?;
            let value = aggregate::format_value(res.func, res.value);
            let summary = if deps.ko() {
                match func {
                    AggregateFn::Count => format!("{} 건", value),
                    _ => format!(
                        "{} {}: {}{}",
                        res.field_label.clone().unwrap_or_default(),
                        match func {
                            AggregateFn::Sum => "합계",
                            AggregateFn::Avg => "평균",
                            AggregateFn::Min => "최소",
                            AggregateFn::Max => "최대",
                            AggregateFn::Count => "",
                        },
                        value,
                        if res.unreviewed.count > 0 && !req.include_unreviewed {
                            format!(" (검토 대기 {}건 제외)", res.unreviewed.count)
                        } else {
                            String::new()
                        }
                    ),
                }
            } else {
                format!(
                    "{} of {}: {}",
                    func.as_str(),
                    res.field_label.clone().unwrap_or_else(|| "documents".into()),
                    value
                )
            };
            Ok(StepOutcome { summary, payload: serde_json::to_value(&res).unwrap_or(Value::Null), imported: None })
        }
        Op::Organize { template, folder_template, group_by, only_reviewed, dry_run } => organize_step(
            deps,
            plan,
            imported,
            Some(template.clone().unwrap_or_else(|| organize::NAME_TEMPLATE.into())),
            Some(
                folder_template
                    .clone()
                    .unwrap_or_else(|| organize::folder_template(group_by.unwrap_or(GroupBy::DocType)).into()),
            ),
            only_reviewed.unwrap_or(deps.settings.organize_reviewed_only),
            dry_run.unwrap_or(false),
            "organize",
            progress,
            out,
        ),
        Op::Rename { template, only_reviewed } => organize_step(
            deps,
            plan,
            imported,
            Some(template.clone()),
            None,
            only_reviewed.unwrap_or(deps.settings.organize_reviewed_only),
            false,
            "rename",
            progress,
            out,
        ),
        Op::Move { folder, only_reviewed, .. } => organize_step(
            deps,
            plan,
            imported,
            None,
            Some(folder.clone()),
            only_reviewed.unwrap_or(deps.settings.organize_reviewed_only),
            false,
            "move",
            progress,
            out,
        ),
        Op::Tag { add, remove } => {
            let docs_ = working_set(deps, plan, imported)?;
            for d in &docs_ {
                let mut tags: Vec<String> =
                    d.tags.iter().filter(|t| !remove.iter().any(|r| r.eq_ignore_ascii_case(t))).cloned().collect();
                for a in add {
                    if !tags.iter().any(|t| t.eq_ignore_ascii_case(a)) {
                        tags.push(a.clone());
                    }
                }
                db.with(|c| docs::set_tags(c, &d.id, &tags))?;
                out.documents.push(d.id.clone());
            }
            Ok(StepOutcome {
                summary: if deps.ko() {
                    format!("{}에 태그 적용(+{} −{})", count_label(deps, docs_.len()), add.len(), remove.len())
                } else {
                    format!("Tagged {} (+{} −{})", count_label(deps, docs_.len()), add.len(), remove.len())
                },
                payload: json!({ "documents": ids(&docs_) }),
                imported: None,
            })
        }
        Op::Delete { .. } => {
            let docs_ = working_set(deps, plan, imported)?;
            let label = if deps.ko() {
                format!("{} 삭제", count_label(deps, docs_.len()))
            } else {
                format!("Delete {}", count_label(deps, docs_.len()))
            };
            let report = organize::delete_documents(&deps.import, &ids(&docs_), &label, "planner", deps.cancel)?;
            out.documents.extend(report.documents.iter().cloned());
            if let Some(b) = report.batch_id.clone() {
                out.batches.push(b);
            }
            if report.cancelled {
                return Err(CoreError::Cancelled);
            }
            Ok(StepOutcome {
                summary: if deps.ko() {
                    format!("{}건 휴지통으로 이동", report.moved)
                } else {
                    format!("{} moved to the recycle bin", report.moved)
                },
                payload: serde_json::to_value(&report).unwrap_or(Value::Null),
                imported: None,
            })
        }
        Op::Export { format, path, columns, layout, include, target } => {
            let Some(step_scope) = step_scope(plan, imported) else { return Ok(nothing_in_scope(deps)) };
            let mut req = ExportRequest {
                format: *format,
                target: target.unwrap_or(ExportTarget::Fields),
                path: path.clone(),
                columns: columns.clone(),
                layout: layout.unwrap_or(ExportLayout::SheetPerType),
                include: include.clone(),
                scope: step_scope,
                only_reviewed: false,
                overwrite: true,
                language: deps.language,
            };
            // A default (unnamed) target that already exists gets a fresh name instead of being
            // overwritten: the card only promised an overwrite for an explicit path.
            if path.is_none() {
                let preview = export::preview(db, deps.paths(), &req)?;
                if preview.exists {
                    let (stem, ext) = match preview.path.rsplit_once('.') {
                        Some((s, e)) => (s.to_string(), format!(".{e}")),
                        None => (preview.path.clone(), String::new()),
                    };
                    let mut n = 2;
                    loop {
                        let candidate = format!("{stem}_{n}{ext}");
                        if !deps.paths().root.join(&candidate).exists() || n > 999 {
                            req.path = Some(candidate);
                            break;
                        }
                        n += 1;
                    }
                }
            }
            let res = export::export(db, deps.paths(), &req, deps.cancel)?;
            Ok(StepOutcome {
                summary: format!("{} → {}", count_label(deps, res.documents as usize), res.path),
                payload: serde_json::to_value(&res).unwrap_or(Value::Null),
                imported: None,
            })
        }
        Op::Undo { target } => {
            let report = match target.as_deref() {
                Some(id) => Some(journal::undo_batch(db, deps.paths(), id)?),
                None => journal::undo_last(db, deps.paths())?,
            };
            match report {
                None => Ok(StepOutcome {
                    summary: deps.t("되돌릴 작업이 없습니다", "Nothing to undo").into(),
                    payload: Value::Null,
                    imported: None,
                }),
                Some(r) => {
                    out.documents.extend(r.documents.iter().cloned());
                    out.restored_paths.extend(r.restored_paths.iter().cloned());
                    out.batches.push(r.batch_id.clone());
                    Ok(StepOutcome {
                        summary: if deps.ko() {
                            format!(
                                "{}건 되돌림{}",
                                r.restored,
                                if r.skipped.is_empty() {
                                    String::new()
                                } else {
                                    format!(" · {}건 불가", r.skipped.len())
                                }
                            )
                        } else {
                            format!(
                                "{} restored{}",
                                r.restored,
                                if r.skipped.is_empty() {
                                    String::new()
                                } else {
                                    format!(" · {} skipped", r.skipped.len())
                                }
                            )
                        },
                        payload: serde_json::to_value(&r).unwrap_or(Value::Null),
                        imported: None,
                    })
                }
            }
        }
        Op::Redo {} => match journal::redo_last(db, deps.paths())? {
            None => Ok(StepOutcome {
                summary: deps.t("다시 실행할 작업이 없습니다", "Nothing to redo").into(),
                payload: Value::Null,
                imported: None,
            }),
            Some(r) => {
                out.documents.extend(r.documents.iter().cloned());
                out.batches.push(r.batch_id.clone());
                Ok(StepOutcome {
                    summary: if deps.ko() {
                        format!("{}건 다시 실행", r.restored)
                    } else {
                        format!("{} redone", r.restored)
                    },
                    payload: serde_json::to_value(&r).unwrap_or(Value::Null),
                    imported: None,
                })
            }
        },
        Op::DraftDocument { form_ref, format_description, content, sources, output, tone, length } => {
            let chat = deps.chat.ok_or_else(|| no_model(deps))?;
            let model = deps.model.clone().ok_or_else(|| no_model(deps))?;
            let _lease = LeaseGuard::acquire(deps.lease);
            let vlm = Vlm::new(chat, model, deps.profile, deps.cancel).with_context(deps.ctx_size);
            let mut srcs: Vec<SourceRef> = Vec::new();
            for s in sources {
                match s.as_str() {
                    "selection" => {
                        for id in &plan.scope.doc_ids {
                            srcs.push(SourceRef::Document { doc_id: id.clone() });
                        }
                    }
                    "aggregate" => {
                        if let Some(v) = out
                            .steps
                            .iter()
                            .rev()
                            .find(|st| st.op == OpKind::Aggregate && st.status == StepStatus::Done)
                            .map(|st| st.payload.clone())
                        {
                            srcs.push(SourceRef::Aggregate { result: v });
                        }
                    }
                    id => srcs.push(SourceRef::Document { doc_id: id.to_string() }),
                }
            }
            if srcs.is_empty() && !plan.scope.is_empty() {
                for d in working_set(deps, plan, imported)?.iter().take(10) {
                    srcs.push(SourceRef::Document { doc_id: d.id.clone() });
                }
            }
            let req = DraftRequest {
                form_ref: form_ref.clone(),
                format_description: format_description.clone(),
                content: content.clone(),
                sources: srcs,
                output: output.unwrap_or_default(),
                tone: tone.unwrap_or_default(),
                length: length.unwrap_or_default(),
                language: deps.language,
                // A draft asked for inside a project looks its reference material up in that project
                // (work documents ∪ reference group ∪ 공통 참고 자료), not in the whole library (audit CON-3).
                project_id: plan.scope.filter.project_id.clone(),
                ..DraftRequest::default()
            };
            let gdeps = GenerateDeps {
                db,
                paths: deps.paths(),
                vlm: Some(&vlm),
                knowledge: Some(SearchDeps { db, analyzer: deps.analyzer, embedder: deps.embedder }),
                printer: Some(deps.import.printer),
                import: Some(&deps.import),
                cancel: deps.cancel,
                language: deps.language,
                progress: None,
            };
            progress("draft", 0, 1, content);
            let outcome = generate::draft(&gdeps, &req)?;
            let _ = generate::store::save(deps.paths(), &outcome.draft);
            let summary = if deps.ko() {
                format!("'{}' 초안 생성 — 문서 생성 화면에서 확인·저장", outcome.draft.title)
            } else {
                format!("Drafted '{}' — review and save in the document generator", outcome.draft.title)
            };
            Ok(StepOutcome { summary, payload: serde_json::to_value(&outcome).unwrap_or(Value::Null), imported: None })
        }
        // 0.7.0 plan §1.5: revise a stored draft (the one open in the generator unless `draft_id` says otherwise).
        Op::ReviseDraft { draft_id, scope, section, heading, instruction } => {
            let chat = deps.chat.ok_or_else(|| no_model(deps))?;
            let model = deps.model.clone().ok_or_else(|| no_model(deps))?;
            let id = draft_id.clone().ok_or_else(|| CoreError::Invalid("revise_draft needs a draft id".into()))?;
            // Taken before the draft is read: a version stored after this moment is newer than the
            // one this revision starts from.
            let captured_at = chrono::Utc::now();
            let previous = generate::store::load(deps.paths(), &id)?;
            let revise = match scope.as_str() {
                "section" => ReviseScope::Section { section: section.unwrap_or(1).max(1) as usize },
                "insert_after" => ReviseScope::InsertAfter {
                    section: section.map(|n| n as usize).filter(|n| *n >= 1),
                    heading: heading.clone(),
                },
                _ => ReviseScope::Whole,
            };
            let _lease = LeaseGuard::acquire(deps.lease);
            let vlm = Vlm::new(chat, model, deps.profile, deps.cancel).with_context(deps.ctx_size);
            let base = previous.request.as_deref().cloned().unwrap_or_default();
            let req = DraftRequest {
                language: previous.language,
                previous: Some(Box::new(previous)),
                feedback: Some(instruction.clone()),
                revise: Some(revise),
                ..base
            };
            let gdeps = GenerateDeps {
                db,
                paths: deps.paths(),
                vlm: Some(&vlm),
                knowledge: Some(SearchDeps { db, analyzer: deps.analyzer, embedder: deps.embedder }),
                printer: Some(deps.import.printer),
                import: Some(&deps.import),
                cancel: deps.cancel,
                language: deps.language,
                progress: None,
            };
            progress("revise_draft", 0, 1, instruction);
            let mut outcome = generate::draft(&gdeps, &req)?;
            // The result replaces the stored draft only when that loses nothing (audit GEN-3). After
            // a cancel — a cancelled "전체 다시 쓰기" returns the sections written so far — or when the
            // draft was edited while the revision ran, it is stored as a new draft and the original
            // stays; it used to be saved over the complete draft unconditionally.
            let cancelled = deps.cancelled();
            let input = generate::store::JobInput { previous: req.previous.as_deref(), captured_at, existed: true };
            let stored = generate::store::save_job_result(
                deps.paths(),
                &mut outcome.draft,
                &input,
                cancelled,
                req.language == Language::Ko,
            )
            .unwrap_or(generate::store::JobSave::NotSaved { reason: generate::store::SkipReason::Failed });
            if cancelled {
                return Err(CoreError::Cancelled);
            }
            let what = match scope.as_str() {
                "section" => {
                    if deps.ko() {
                        format!("{}절", section.unwrap_or(1))
                    } else {
                        format!("section {}", section.unwrap_or(1))
                    }
                }
                "insert_after" => {
                    if deps.ko() {
                        "새 절".to_string()
                    } else {
                        "a new section".to_string()
                    }
                }
                _ => {
                    if deps.ko() {
                        "전체".to_string()
                    } else {
                        "the whole draft".to_string()
                    }
                }
            };
            let mut summary = if deps.ko() {
                format!("'{}' 초안 {what} AI 수정 — 문서 생성 화면에서 확인·저장", outcome.draft.title)
            } else {
                format!("Revised {what} of '{}' — review and save in the document generator", outcome.draft.title)
            };
            {
                use generate::store::{JobSave, SkipReason};
                summary.push_str(match (&stored, deps.ko()) {
                    (JobSave::Saved, _) => "",
                    (JobSave::Copy { .. }, true) => " · 원래 초안이 그 사이 수정되어 새 초안으로 저장",
                    (JobSave::Copy { .. }, false) => " · stored as a new draft (the original was edited meanwhile)",
                    (JobSave::NotSaved { reason: SkipReason::Deleted }, true) => {
                        " · 초안이 삭제되어 목록에 저장하지 않음"
                    }
                    (JobSave::NotSaved { reason: SkipReason::Deleted }, false) => {
                        " · not stored (the draft was deleted)"
                    }
                    (JobSave::NotSaved { reason: SkipReason::Failed }, true) => " · 초안 목록에 저장하지 못함",
                    (JobSave::NotSaved { reason: SkipReason::Failed }, false) => " · could not be stored",
                });
            }
            // Serialised after the save: a copy has a new id. `draft_store` says where the result went
            // (`{status: saved}` | `{status: copy, original, reason}` | `{status: not_saved, reason}`),
            // as the result of a draft job does.
            let mut payload = serde_json::to_value(&outcome).unwrap_or(Value::Null);
            if let Some(o) = payload.as_object_mut() {
                o.insert("draft_store".into(), serde_json::to_value(&stored).unwrap_or(Value::Null));
            }
            Ok(StepOutcome { summary, payload, imported: None })
        }
        Op::Explain { selection, question, scope: explain_scope, allow_general_knowledge } => {
            let chat = deps.chat.ok_or_else(|| no_model(deps))?;
            let _lease = LeaseGuard::acquire(deps.lease);
            let mut req = AnswerRequest::question(question.as_deref().unwrap_or(""));
            req.kind = AnswerKind::Explain;
            req.selection = selection.clone();
            match explain_scope {
                Some(ExplainScope::Document) => {
                    req.document_id = plan.scope.doc_ids.first().cloned();
                    req.scope.documents = plan.scope.doc_ids.clone();
                }
                _ => {
                    if !plan.scope.is_empty() || imported.is_some() {
                        req.scope.documents = ids(&working_set(deps, plan, imported)?);
                        if req.scope.documents.is_empty() && scope_narrows(plan, imported) {
                            return Ok(no_documents_answer(
                                deps,
                                question.as_deref().unwrap_or(""),
                                AnswerKind::Explain,
                            ));
                        }
                    }
                    req.scope.project_id = plan.scope.filter.project_id.clone();
                }
            }
            req.scope.latest_only = deps.knowledge.latest_only;
            req.allow_general = allow_general_knowledge.unwrap_or(deps.knowledge.allow_general_knowledge);
            req.top_k = deps.knowledge.top_k;
            req.context_tokens = deps.knowledge.context_tokens;
            req.min_similarity = Some(deps.knowledge.min_vector_similarity);
            let cancelled = || deps.cancelled();
            let adeps = AnswerDeps {
                search: SearchDeps { db, analyzer: deps.analyzer, embedder: deps.embedder },
                chat,
                model: deps.model.clone(),
                cancelled: Some(&cancelled),
                context_size: deps.ctx_size,
            };
            let ans = answer::answer(&adeps, &req, &mut |_| !deps.cancelled())?;
            Ok(StepOutcome {
                // An answer given from general knowledge has a text of its own to show.
                summary: if ans.no_evidence && !ans.used_general_knowledge {
                    deps.t("근거를 찾지 못했습니다", "No evidence found").into()
                } else {
                    crate::pipeline::prompts::truncate(&ans.text, 200)
                },
                payload: serde_json::to_value(&ans).unwrap_or(Value::Null),
                imported: None,
            })
        }
        Op::Compare { doc_ids, aspects } => {
            let chat = deps.chat.ok_or_else(|| no_model(deps))?;
            let _lease = LeaseGuard::acquire(deps.lease);
            // The knowledge comparer works on a pair; further documents are compared against the first.
            let mut req = AnswerRequest::question(&aspects.join(", "));
            req.kind = AnswerKind::Compare;
            req.document_id = doc_ids.first().cloned();
            req.compare_document_id = doc_ids.get(1).cloned();
            req.scope.documents = doc_ids.clone();
            req.context_tokens = deps.knowledge.context_tokens;
            let cancelled = || deps.cancelled();
            let adeps = AnswerDeps {
                search: SearchDeps { db, analyzer: deps.analyzer, embedder: deps.embedder },
                chat,
                model: deps.model.clone(),
                cancelled: Some(&cancelled),
                context_size: deps.ctx_size,
            };
            let ans = answer::compare(&adeps, &req, &mut |_| !deps.cancelled())?;
            Ok(StepOutcome {
                summary: crate::pipeline::prompts::truncate(&ans.text, 200),
                payload: serde_json::to_value(&ans).unwrap_or(Value::Null),
                imported: None,
            })
        }
        Op::CreateProject { name, template, doc_types_required, start_date, end_date } => {
            let templates = db.with(projects_repo::list_templates)?;
            let mut template_id = template.as_deref().and_then(|t| {
                templates
                    .iter()
                    .find(|tp| tp.id == t || compact(&tp.name_ko) == compact(t) || compact(&tp.name_en) == compact(t))
                    .map(|tp| tp.id.clone())
            });
            if template_id.is_none() && !doc_types_required.is_empty() {
                // Ad-hoc checklist: a user template named after the project.
                let tp = db.with(|c| {
                    projects_repo::save_template(
                        c,
                        &TemplateInput {
                            name_ko: name.clone(),
                            required_doc_types: doc_types_required.clone(),
                            ..TemplateInput::default()
                        },
                    )
                })?;
                template_id = Some(tp.id);
            }
            let input = ProjectInput {
                name: name.clone(),
                template_id,
                start_date: start_date.clone(),
                end_date: end_date.clone(),
                ..ProjectInput::default()
            };
            let project = db.with(|c| projects_repo::save_project(c, &input))?;
            let mut added = 0u32;
            // Only documents the plan names (or an earlier step imported) join the new project;
            // the validator no longer fills the open project in as a default scope (audit PLN-5).
            if !plan.scope.is_empty() || imported.is_some() {
                let docs_ = working_set(deps, plan, imported)?;
                if !docs_.is_empty() {
                    added = crate::projects::copies::add_documents(
                        db,
                        deps.paths(),
                        &project.id,
                        &ids(&docs_),
                        deps.language,
                    )?;
                    out.documents.extend(ids(&docs_));
                }
            }
            let summary = if deps.ko() {
                format!(
                    "'{}' 프로젝트 생성{}",
                    project.name,
                    if added > 0 { format!(" · 문서 {added}건 추가") } else { String::new() }
                )
            } else {
                format!(
                    "Created project '{}'{}",
                    project.name,
                    if added > 0 { format!(" · {added} documents added") } else { String::new() }
                )
            };
            Ok(StepOutcome { summary, payload: serde_json::to_value(&project).unwrap_or(Value::Null), imported: None })
        }
        Op::AddToProject { project, doc_ids, as_reference } => {
            let pid = project.clone().ok_or_else(|| CoreError::Invalid("project_required".into()))?;
            let target = if doc_ids.is_empty() { ids(&working_set(deps, plan, imported)?) } else { doc_ids.clone() };
            if target.is_empty() {
                return Err(CoreError::Invalid("no_documents".into()));
            }
            let added = if *as_reference {
                // Reference material (0.5.0 plan A): the project's reference group + role `knowledge`.
                db.with_tx(|tx| {
                    let name = projects_repo::get_project(tx, &pid)?.map(|p| p.name).unwrap_or_default();
                    let group = knowledge_repo::project_reference_group(tx, &pid, &name)?;
                    let n = knowledge_repo::add_docs_to_collection(tx, &group.id, &target)?;
                    for d in &target {
                        docs::set_role(tx, d, DocumentRole::Knowledge)?;
                    }
                    Ok(n)
                })?
            } else {
                crate::projects::copies::add_documents(db, deps.paths(), &pid, &target, deps.language)?
            };
            out.documents.extend(target.iter().cloned());
            let detail = db.with(|c| projects_repo::get_project(c, &pid))?;
            Ok(StepOutcome {
                summary: match (deps.ko(), *as_reference) {
                    (true, true) => format!("프로젝트 참고 자료에 문서 {added}건 추가"),
                    (true, false) => format!("프로젝트에 문서 {added}건 추가"),
                    (false, true) => format!("{added} documents added to the project's reference material"),
                    (false, false) => format!("{added} documents added to the project"),
                },
                payload: json!({ "project_id": pid, "added": added, "as_reference": as_reference, "project": detail }),
                imported: None,
            })
        }
        Op::DefineRule { project, template, description } => {
            // Without a project the rule goes to the named template (id or name), resolved up
            // front: a built-in template takes no rules (`template_builtin`, as `delete_template`).
            let target_template = match (project.as_deref(), template.as_deref()) {
                (None, Some(key)) => {
                    let t = resolve_template(db, key)?.ok_or_else(|| CoreError::NotFound(format!("template {key}")))?;
                    if t.builtin {
                        return Err(CoreError::Invalid("template_builtin".into()));
                    }
                    Some(t)
                }
                _ => None,
            };
            let template_key = target_template.as_ref().map(|t| t.id.clone()).or_else(|| template.clone());
            let vlm = match (deps.chat, deps.model.clone()) {
                (Some(c), Some(m)) => Some(Vlm::new(c, m, deps.profile, deps.cancel).with_context(deps.ctx_size)),
                _ => None,
            };
            let _lease = vlm.as_ref().map(|_| LeaseGuard::acquire(deps.lease));
            let pdeps = ProjectDeps { db, vlm: vlm.as_ref(), language: deps.language };
            let draft = projects::define_rule(&pdeps, project.as_deref(), template_key.as_deref(), description)?;
            let confident = draft.confidence >= 0.6 && draft.unresolved.is_empty() && draft.issues.is_empty();
            let mut saved = false;
            let mut template_id = None;
            if confident {
                if let Some(pid) = project.as_deref() {
                    db.with(|c| projects_repo::save_rule(c, pid, &draft.rule))?;
                    saved = true;
                } else if let Some(t) = target_template {
                    let mut rules = t.rules.clone();
                    rules.push(draft.rule.clone());
                    db.with(|c| {
                        projects_repo::save_template(
                            c,
                            &TemplateInput {
                                id: Some(t.id.clone()),
                                name_ko: t.name_ko.clone(),
                                name_en: Some(t.name_en.clone()),
                                description_ko: Some(t.description_ko.clone()),
                                description_en: Some(t.description_en.clone()),
                                required_doc_types: t.required_doc_types.clone(),
                                rules,
                            },
                        )
                    })?;
                    saved = true;
                    template_id = Some(t.id);
                }
            }
            let summary = match (saved, template_id.is_some(), deps.ko()) {
                (true, true, true) => format!("템플릿에 규칙 저장: {}", draft.rule.description),
                (true, true, false) => format!("Rule saved to the template: {}", draft.rule.description),
                (true, false, true) => format!("규칙 저장: {}", draft.rule.description),
                (true, false, false) => format!("Rule saved: {}", draft.rule.description),
                (false, _, true) => "규칙 초안 — 프로젝트 화면의 규칙 그리드에서 확인하세요".to_string(),
                (false, _, false) => "Rule draft — confirm it in the project's rule grid".to_string(),
            };
            Ok(StepOutcome {
                summary,
                payload: json!({ "project_id": project, "template_id": template_id, "saved": saved, "draft": draft }),
                imported: None,
            })
        }
        Op::CheckConsistency { project, rules } => {
            let pid = project.clone().ok_or_else(|| CoreError::Invalid("project_required".into()))?;
            // The model only judges fuzzy name comparisons; without it they stay undetermined.
            let vlm = match (deps.chat, deps.model.clone()) {
                (Some(c), Some(m)) => Some(Vlm::new(c, m, deps.profile, deps.cancel).with_context(deps.ctx_size)),
                _ => None,
            };
            let _lease = vlm.as_ref().map(|_| LeaseGuard::acquire(deps.lease));
            let pdeps = ProjectDeps { db, vlm: vlm.as_ref(), language: deps.language };
            // A reference that matches no rule fails here: the check would otherwise run zero
            // rules and that empty run would become the project's latest check (audit PRJ-12).
            let rule_ids = resolve_rules(db, &pid, rules)?;
            let run = projects::check(&pdeps, &pid, &rule_ids)?;
            let mut summary = if deps.ko() {
                format!(
                    "통과 {} · 불일치 {} · 판단불가 {} · 누락 {}",
                    run.pass, run.mismatch, run.undetermined, run.missing
                )
            } else {
                format!(
                    "pass {} · mismatch {} · undetermined {} · missing {}",
                    run.pass, run.mismatch, run.undetermined, run.missing
                )
            };
            if !rule_ids.is_empty() {
                summary.push_str(&if deps.ko() {
                    format!(" (규칙 {}개만 검사)", rule_ids.len())
                } else {
                    format!(" ({} rules only)", rule_ids.len())
                });
            }
            Ok(StepOutcome { summary, payload: serde_json::to_value(&run).unwrap_or(Value::Null), imported: None })
        }
        Op::RequestSupplement { project, findings, output } => {
            let pid = project.clone().ok_or_else(|| CoreError::Invalid("project_required".into()))?;
            let run = db
                .with(|c| projects_repo::last_run(c, &pid))?
                .ok_or_else(|| CoreError::Invalid("no_check_run".into()))?;
            let mut req =
                projects::supplement_request(db, &pid, &run, findings, output.unwrap_or_default(), deps.language)?;
            // The request belongs to the project it is written for: knowledge searches of the draft
            // (and of a later regeneration from the stored request) stay inside it (audit CON-3).
            req.bind_project(Some(pid.clone()));
            // Findings as text sources → the rule-based 보완요청서 needs no model (the shell's draft
            // job decides with the same predicate), so the planner never waits for the engine here.
            let rule_only = generate::draft::rule_only(&req);
            let vlm = match (deps.chat, deps.model.clone()) {
                (Some(c), Some(m)) if !rule_only => {
                    Some(Vlm::new(c, m, deps.profile, deps.cancel).with_context(deps.ctx_size))
                }
                _ => None,
            };
            let _lease = vlm.as_ref().map(|_| LeaseGuard::acquire(deps.lease));
            let gdeps = GenerateDeps {
                db,
                paths: deps.paths(),
                vlm: vlm.as_ref(),
                knowledge: None,
                printer: Some(deps.import.printer),
                import: Some(&deps.import),
                cancel: deps.cancel,
                language: deps.language,
                progress: None,
            };
            let outcome = generate::draft(&gdeps, &req)?;
            let _ = generate::store::save(deps.paths(), &outcome.draft);
            let mut payload = serde_json::to_value(&outcome).unwrap_or(Value::Null);
            if let Some(o) = payload.as_object_mut() {
                o.insert("project_id".into(), json!(pid));
            }
            Ok(StepOutcome {
                summary: if deps.ko() {
                    "보완요청서 초안 생성 — 문서 생성 화면에서 확인·저장".into()
                } else {
                    "Supplement request drafted — review and save in the document generator".into()
                },
                payload,
                imported: None,
            })
        }
        // -- 0.6.0: every GUI function as an op ----------------------------------------------
        Op::Rotate { degrees, pages } => {
            let docs_ = non_empty(working_set(deps, plan, imported)?)?;
            let mut rotated = 0u32;
            for d in &docs_ {
                deps.checkpoint()?;
                // A relative turn: the new rotation is the page's current one plus `degrees`.
                let rows = db.with(|c| docs::list_pages(c, &d.id, false))?;
                for p in rows.iter().filter(|p| pages.is_empty() || pages.contains(&p.page_no)) {
                    import::rotate_page(&deps.import, d, p.page_no, (p.rotation + degrees) % 360)?;
                    rotated += 1;
                }
                out.documents.push(d.id.clone());
            }
            Ok(StepOutcome {
                summary: if deps.ko() {
                    format!("{} {degrees}° 회전({rotated}쪽)", count_label(deps, docs_.len()))
                } else {
                    format!("Rotated {} by {degrees}° ({rotated} pages)", count_label(deps, docs_.len()))
                },
                payload: json!({ "documents": ids(&docs_), "pages": rotated, "degrees": degrees }),
                imported: None,
            })
        }
        Op::Rerender { dpi } => {
            let docs_ = non_empty(working_set(deps, plan, imported)?)?;
            let total = docs_.len() as u32;
            for (i, d) in docs_.iter().enumerate() {
                deps.checkpoint()?;
                progress("render", i as u32, total, &d.original_name);
                import::process_document(&deps.import, d, true, *dpi, deps.cancel, &mut |_| {})?;
                out.documents.push(d.id.clone());
            }
            Ok(StepOutcome {
                summary: if deps.ko() {
                    format!(
                        "{} 다시 렌더{}",
                        count_label(deps, docs_.len()),
                        dpi.map(|d| format!(" ({d} DPI)")).unwrap_or_default()
                    )
                } else {
                    format!(
                        "Re-rendered {}{}",
                        count_label(deps, docs_.len()),
                        dpi.map(|d| format!(" at {d} DPI")).unwrap_or_default()
                    )
                },
                payload: json!({ "documents": ids(&docs_), "dpi": dpi }),
                imported: None,
            })
        }
        Op::Split { boundaries, every } => {
            let doc = one_document(working_set(deps, plan, imported)?, "split_needs_one_document")?;
            let bounds = split_boundaries(doc.page_count, boundaries, *every)?;
            let created = split::split_document(&deps.import, &doc, &bounds, deps.cancel)?;
            out.documents.extend(created.iter().cloned());
            Ok(StepOutcome {
                summary: if deps.ko() {
                    format!("{} {}개 문서로 분할", josa::eul_reul(&format!("'{}'", doc.original_name)), created.len())
                } else {
                    format!("Split '{}' into {} documents", doc.original_name, created.len())
                },
                payload: json!({ "document_id": doc.id, "created": created, "boundaries": bounds }),
                imported: None,
            })
        }
        Op::Merge { name } => {
            let docs_ = working_set(deps, plan, imported)?;
            if docs_.len() < 2 || docs_.len() > 20 {
                return Err(CoreError::Invalid("merge_needs_2_to_20_documents".into()));
            }
            let id = split::merge_documents(&deps.import, &docs_, name.as_deref(), deps.cancel)?;
            out.documents.push(id.clone());
            Ok(StepOutcome {
                summary: if deps.ko() {
                    format!("{} 하나로 병합", count_label(deps, docs_.len()))
                } else {
                    format!("Merged {} into one document", count_label(deps, docs_.len()))
                },
                payload: json!({ "document_id": id, "sources": ids(&docs_) }),
                imported: None,
            })
        }
        Op::SetTitle { title } => {
            let doc = one_document(working_set(deps, plan, imported)?, "set_title_needs_one_document")?;
            db.with(|c| docs::set_title(c, &doc.id, Some(title.trim())))?;
            out.documents.push(doc.id.clone());
            Ok(StepOutcome {
                summary: if deps.ko() {
                    format!("제목을 {} 바꿈", josa::euro_ro(&format!("'{}'", title.trim())))
                } else {
                    format!("Title set to '{}'", title.trim())
                },
                payload: json!({ "documents": [doc.id], "title": title.trim() }),
                imported: None,
            })
        }
        Op::EditText { find, replace, pages } => {
            let docs_ = non_empty(working_set(deps, plan, imported)?)?;
            let (mut replaced, mut touched) = (0usize, 0usize);
            for d in &docs_ {
                deps.checkpoint()?;
                let rows = db.with(|c| docs::list_pages(c, &d.id, true))?;
                for p in rows.iter().filter(|p| pages.is_empty() || pages.contains(&p.page_no)) {
                    let Some(text) = p.text.as_deref() else { continue };
                    let n = text.matches(find.as_str()).count();
                    if n == 0 {
                        continue;
                    }
                    if import::set_page_text(db, &d.id, p.page_no, &text.replace(find.as_str(), replace))? {
                        replaced += n;
                        touched += 1;
                    }
                }
                out.documents.push(d.id.clone());
            }
            Ok(StepOutcome {
                summary: if deps.ko() {
                    format!("'{find}' → '{replace}' {replaced}곳 바꿈({touched}쪽)")
                } else {
                    format!("Replaced '{find}' with '{replace}' {replaced} times ({touched} pages)")
                },
                payload: json!({ "documents": ids(&docs_), "replaced": replaced, "pages": touched }),
                imported: None,
            })
        }
        Op::SetRole { role } => {
            let docs_ = non_empty(working_set(deps, plan, imported)?)?;
            let target = role.as_role();
            // A document that is still in a reference group stays reference material when it is asked
            // to become a work document (audit PRJ-4): the group decides, the summary says how many.
            let kept = db.with_tx(|tx| {
                let mut kept = 0usize;
                for d in &docs_ {
                    if knowledge_repo::set_role_by_membership(tx, &d.id, target)? != target {
                        kept += 1;
                    }
                }
                Ok(kept)
            })?;
            out.documents.extend(ids(&docs_));
            let changed = docs_.len() - kept;
            let note = match (kept, deps.ko()) {
                (0, _) => String::new(),
                (n, true) => format!(" (참고 자료 묶음에 들어 있는 {n}건은 참고 자료로 유지)"),
                (n, false) => format!(" ({n} still in a reference group stay reference material)"),
            };
            Ok(StepOutcome {
                summary: if changed == 0 {
                    // Nothing changed: say why instead of "0건".
                    if deps.ko() {
                        format!("참고 자료 묶음에 들어 있는 문서라 참고 자료로 둡니다({kept}건). 묶음에서 빼면 업무 문서가 됩니다")
                    } else {
                        format!("Still in a reference group: kept as reference material ({kept}). Take them out of the group to make them work documents")
                    }
                } else if deps.ko() {
                    format!("{} 역할: {}{note}", count_label(deps, changed), role_label(target, true))
                } else {
                    format!("{} marked as {}{note}", count_label(deps, changed), role_label(target, false))
                },
                payload: json!({ "documents": ids(&docs_), "role": target }),
                imported: None,
            })
        }
        Op::ManageReference { action, group, name, project } => {
            let ko = deps.ko();
            match action {
                ReferenceAction::Create => {
                    let n = name
                        .clone()
                        .or_else(|| group.clone())
                        .ok_or_else(|| CoreError::Invalid("name_required".into()))?;
                    let col = db.with(|c| {
                        knowledge_repo::save_collection(
                            c,
                            &CollectionInput { name: n.clone(), active: true, ..CollectionInput::default() },
                        )
                    })?;
                    Ok(StepOutcome {
                        summary: if ko {
                            format!("참고 자료 묶음 '{}' 만듦", col.name)
                        } else {
                            format!("Reference group '{}' created", col.name)
                        },
                        payload: json!({ "action": action, "group": col, "documents": [] }),
                        imported: None,
                    })
                }
                ReferenceAction::Rename
                | ReferenceAction::Enable
                | ReferenceAction::Disable
                | ReferenceAction::Delete => {
                    let col = resolve_group(db, group.as_deref(), project.as_deref())?.ok_or_else(|| {
                        CoreError::NotFound(if ko { "참고 자료 묶음".into() } else { "reference group".into() })
                    })?;
                    let (col, summary) = match action {
                        ReferenceAction::Delete => {
                            db.with(|c| knowledge_repo::delete_collection(c, &col.id))?;
                            let s = if ko {
                                format!("참고 자료 묶음 '{}' 삭제(문서는 남음)", col.name)
                            } else {
                                format!("Reference group '{}' deleted (documents kept)", col.name)
                            };
                            (None, s)
                        }
                        _ => {
                            let new_name = if *action == ReferenceAction::Rename {
                                name.clone().ok_or_else(|| CoreError::Invalid("name_required".into()))?
                            } else {
                                col.name.clone()
                            };
                            let active = match action {
                                ReferenceAction::Enable => true,
                                ReferenceAction::Disable => false,
                                _ => col.active,
                            };
                            let saved = db.with(|c| {
                                knowledge_repo::save_collection(
                                    c,
                                    &CollectionInput {
                                        id: Some(col.id.clone()),
                                        name: new_name,
                                        description: col.description.clone(),
                                        color: col.color.clone(),
                                        active,
                                        project_id: None,
                                    },
                                )
                            })?;
                            let s = match action {
                                ReferenceAction::Rename => {
                                    if ko {
                                        format!("묶음 이름을 {} 바꿈", josa::euro_ro(&format!("'{}'", saved.name)))
                                    } else {
                                        format!("Group renamed to '{}'", saved.name)
                                    }
                                }
                                ReferenceAction::Enable => {
                                    if ko {
                                        format!("'{}' 묶음 사용", saved.name)
                                    } else {
                                        format!("Group '{}' enabled", saved.name)
                                    }
                                }
                                _ => {
                                    if ko {
                                        format!("'{}' 묶음 사용 안 함", saved.name)
                                    } else {
                                        format!("Group '{}' disabled", saved.name)
                                    }
                                }
                            };
                            (Some(saved), s)
                        }
                    };
                    Ok(StepOutcome {
                        summary,
                        payload: json!({ "action": action, "group": col, "documents": [] }),
                        imported: None,
                    })
                }
                ReferenceAction::Add | ReferenceAction::Remove => {
                    let docs_ = non_empty(working_set(deps, plan, imported)?)?;
                    let col = resolve_group(db, group.as_deref(), project.as_deref())?.ok_or_else(|| {
                        CoreError::NotFound(if ko { "참고 자료 묶음".into() } else { "reference group".into() })
                    })?;
                    let doc_ids = ids(&docs_);
                    let n = db.with_tx(|tx| {
                        if *action == ReferenceAction::Add {
                            let n = knowledge_repo::add_docs_to_collection(tx, &col.id, &doc_ids)?;
                            for d in &doc_ids {
                                docs::set_role(tx, d, DocumentRole::Knowledge)?;
                            }
                            Ok(n)
                        } else {
                            knowledge_repo::remove_docs_from_collection(tx, &col.id, &doc_ids)
                        }
                    })?;
                    out.documents.extend(doc_ids.iter().cloned());
                    let group_now = db.with(|c| knowledge_repo::get_collection(c, &col.id))?;
                    Ok(StepOutcome {
                        summary: match (ko, action) {
                            (true, ReferenceAction::Add) => {
                                format!("'{}' 묶음에 문서 {n}건 추가(참고 자료로 표시)", col.name)
                            }
                            (true, _) => format!("'{}' 묶음에서 문서 {n}건 제외", col.name),
                            (false, ReferenceAction::Add) => {
                                format!("{n} documents added to group '{}' (marked as reference material)", col.name)
                            }
                            (false, _) => format!("{n} documents removed from group '{}'", col.name),
                        },
                        payload: json!({ "action": action, "group": group_now, "documents": doc_ids }),
                        imported: None,
                    })
                }
            }
        }
        Op::DefineTerm { action, term, definition, synonyms } => {
            let want = compact(term);
            let existing = db.with(knowledge_repo::list_glossary)?.into_iter().find(|g| compact(&g.term) == want);
            match action {
                TermAction::Save => {
                    let input = GlossaryInput {
                        id: existing.as_ref().map(|g| g.id),
                        term: term.trim().to_string(),
                        definition: definition.clone().or_else(|| existing.as_ref().and_then(|g| g.definition.clone())),
                        synonyms: if synonyms.is_empty() {
                            existing.as_ref().map(|g| g.synonyms.clone()).unwrap_or_default()
                        } else {
                            synonyms.clone()
                        },
                    };
                    let entry = db.with(|c| knowledge_repo::save_glossary(c, &input))?;
                    Ok(StepOutcome {
                        summary: if deps.ko() {
                            format!("용어 '{}' 등록", entry.term)
                        } else {
                            format!("Term '{}' saved", entry.term)
                        },
                        payload: json!({ "action": action, "term": entry }),
                        imported: None,
                    })
                }
                TermAction::Delete => {
                    let g = existing.ok_or_else(|| CoreError::NotFound(format!("term {term}")))?;
                    db.with(|c| knowledge_repo::delete_glossary(c, g.id))?;
                    Ok(StepOutcome {
                        summary: if deps.ko() {
                            format!("용어 '{}' 삭제", g.term)
                        } else {
                            format!("Term '{}' deleted", g.term)
                        },
                        payload: json!({ "action": action, "term": g }),
                        imported: None,
                    })
                }
            }
        }
        Op::SetVersion { group_key, label, effective_date, latest, clear } => {
            let docs_ = non_empty(working_set(deps, plan, imported)?)?;
            if clear.unwrap_or(false) {
                db.with(|c| {
                    for d in &docs_ {
                        knowledge_repo::clear_version(c, &d.id)?;
                    }
                    Ok(())
                })?;
                out.documents.extend(ids(&docs_));
                return Ok(StepOutcome {
                    summary: if deps.ko() {
                        format!("{} 버전 표시 해제", count_label(deps, docs_.len()))
                    } else {
                        format!("Version mark cleared on {}", count_label(deps, docs_.len()))
                    },
                    payload: json!({ "documents": ids(&docs_), "cleared": true }),
                    imported: None,
                });
            }
            let key = group_key
                .clone()
                .filter(|k| !k.trim().is_empty())
                .or_else(|| docs_.first().and_then(|d| d.title.clone()))
                .ok_or_else(|| CoreError::Invalid("group_key_required".into()))?;
            db.with(|c| {
                for d in &docs_ {
                    knowledge_repo::set_version(
                        c,
                        &VersionInput {
                            document_id: d.id.clone(),
                            group_key: key.clone(),
                            version_label: label.clone(),
                            effective_date: effective_date.clone(),
                            is_latest: latest.unwrap_or(true),
                            note: None,
                        },
                    )?;
                }
                Ok(())
            })?;
            out.documents.extend(ids(&docs_));
            Ok(StepOutcome {
                summary: if deps.ko() {
                    format!(
                        "'{key}' 판으로 표시{}{}",
                        label.as_deref().map(|l| format!(" · {l}")).unwrap_or_default(),
                        if latest.unwrap_or(true) { "(최신본)" } else { "(구판)" }
                    )
                } else {
                    format!(
                        "Marked as an edition of '{key}'{}{}",
                        label.as_deref().map(|l| format!(" · {l}")).unwrap_or_default(),
                        if latest.unwrap_or(true) { " (latest)" } else { " (superseded)" }
                    )
                },
                payload: json!({ "documents": ids(&docs_), "group_key": key, "latest": latest.unwrap_or(true) }),
                imported: None,
            })
        }
        Op::UpdateProject {
            project,
            name,
            template,
            start_date,
            end_date,
            owner,
            intake_no,
            notes,
            keywords,
            inbox_folder,
        } => {
            let pid = project.clone().ok_or_else(|| CoreError::Invalid("project_required".into()))?;
            let before = db
                .with(|c| projects_repo::get_project(c, &pid))?
                .ok_or_else(|| CoreError::NotFound(format!("project {pid}")))?;
            let template_id = match template.as_deref().map(str::trim) {
                None => None,
                Some("") => Some(String::new()),
                Some(t) => {
                    Some(resolve_template(db, t)?.ok_or_else(|| CoreError::NotFound(format!("template {t}")))?.id)
                }
            };
            let input = ProjectInput {
                id: Some(pid.clone()),
                name: name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| before.name.clone()),
                template_id,
                intake_no: intake_no.clone(),
                owner: owner.clone(),
                start_date: start_date.clone(),
                end_date: end_date.clone(),
                notes: notes.clone(),
                inbox_folder: inbox_folder.clone(),
                settings: None,
                keywords: keywords.clone(),
            };
            let saved = db.with(|c| projects_repo::save_project(c, &input))?;
            Ok(StepOutcome {
                summary: if deps.ko() {
                    format!("프로젝트 '{}' 수정", saved.name)
                } else {
                    format!("Project '{}' updated", saved.name)
                },
                payload: json!({ "project": saved }),
                imported: None,
            })
        }
        Op::DeleteProject { project } => {
            let pid = project.clone().ok_or_else(|| CoreError::Invalid("project_required".into()))?;
            let before = db
                .with(|c| projects_repo::get_project(c, &pid))?
                .ok_or_else(|| CoreError::NotFound(format!("project {pid}")))?;
            projects::copies::remove_project_copies(db, deps.paths(), &pid)?;
            let deleted = db.with(|c| projects_repo::delete_project(c, &pid))?;
            Ok(StepOutcome {
                summary: if deps.ko() {
                    format!("프로젝트 '{}' 삭제(문서는 문서함에 남음)", before.name)
                } else {
                    format!("Project '{}' deleted (documents stay in the library)", before.name)
                },
                payload: json!({ "project_id": pid, "deleted": deleted, "name": before.name }),
                imported: None,
            })
        }
        Op::RemoveFromProject { project, doc_ids } => {
            let pid = project.clone().ok_or_else(|| CoreError::Invalid("project_required".into()))?;
            let target = if doc_ids.is_empty() { ids(&working_set(deps, plan, imported)?) } else { doc_ids.clone() };
            if target.is_empty() {
                return Err(CoreError::Invalid("no_documents".into()));
            }
            let mut removed = 0u32;
            for d in &target {
                if projects::copies::remove_document(db, deps.paths(), &pid, d)? {
                    removed += 1;
                }
            }
            out.documents.extend(target.iter().cloned());
            Ok(StepOutcome {
                summary: if deps.ko() {
                    format!("프로젝트에서 문서 {removed}건 제외")
                } else {
                    format!("{removed} documents removed from the project")
                },
                payload: json!({ "project_id": pid, "removed": removed, "documents": target }),
                imported: None,
            })
        }
        Op::ManageTemplate { action, template, name, doc_types_required, description } => {
            let key = template
                .clone()
                .or_else(|| name.clone())
                .ok_or_else(|| CoreError::Invalid("template_required".into()))?;
            let existing = resolve_template(db, &key)?;
            match action {
                TemplateAction::Delete => {
                    let t = existing.ok_or_else(|| CoreError::NotFound(format!("template {key}")))?;
                    let ok = db.with(|c| projects_repo::delete_template(c, &t.id))?;
                    Ok(StepOutcome {
                        summary: if deps.ko() {
                            format!("템플릿 '{}' 삭제", t.name_ko)
                        } else {
                            format!("Template '{}' deleted", t.name_en)
                        },
                        payload: json!({ "action": action, "template": t, "deleted": ok }),
                        imported: None,
                    })
                }
                TemplateAction::Save => {
                    if existing.as_ref().is_some_and(|t| t.builtin) {
                        return Err(CoreError::Invalid("template_builtin".into()));
                    }
                    let input = TemplateInput {
                        id: existing.as_ref().map(|t| t.id.clone()),
                        name_ko: name.clone().unwrap_or(key.clone()),
                        name_en: existing.as_ref().map(|t| t.name_en.clone()),
                        description_ko: description
                            .clone()
                            .or_else(|| existing.as_ref().map(|t| t.description_ko.clone())),
                        description_en: existing.as_ref().map(|t| t.description_en.clone()),
                        required_doc_types: if doc_types_required.is_empty() {
                            existing.as_ref().map(|t| t.required_doc_types.clone()).unwrap_or_default()
                        } else {
                            doc_types_required.clone()
                        },
                        rules: existing.as_ref().map(|t| t.rules.clone()).unwrap_or_default(),
                    };
                    let t = db.with(|c| projects_repo::save_template(c, &input))?;
                    Ok(StepOutcome {
                        summary: if deps.ko() {
                            format!("템플릿 '{}' 저장", t.name_ko)
                        } else {
                            format!("Template '{}' saved", t.name_en)
                        },
                        payload: json!({ "action": action, "template": t }),
                        imported: None,
                    })
                }
            }
        }
        Op::ClassifyProjects { apply, use_model } => {
            let scoped = !plan.scope.is_empty() || imported.is_some();
            let docs_ = if scoped { working_set(deps, plan, imported)? } else { Vec::new() };
            if scoped && docs_.is_empty() {
                // No document ids means "every unassigned document" to the classifier: a scope
                // that matches nothing must classify nothing (audit PLN-19).
                return Ok(StepOutcome {
                    summary: deps.t("대상 문서 없음", "No documents in scope").into(),
                    payload: json!({ "suggested": 0, "undecided": 0, "applied": 0, "suggestions": [], "apply_result": null }),
                    imported: None,
                });
            }
            let use_model = use_model.unwrap_or(false) && deps.chat.is_some() && deps.model.is_some();
            let vlm = match (use_model, deps.chat, deps.model.clone()) {
                (true, Some(c), Some(m)) => Some(Vlm::new(c, m, deps.profile, deps.cancel).with_context(deps.ctx_size)),
                _ => None,
            };
            let _lease = vlm.as_ref().map(|_| LeaseGuard::acquire(deps.lease));
            let pdeps = ProjectDeps { db, vlm: vlm.as_ref(), language: deps.language };
            let req =
                ClassifyRequest { doc_ids: ids(&docs_), project_ids: Vec::new(), use_model, include_assigned: false };
            let result = classify::suggest(&pdeps, &req)?;
            let assignments: Vec<Assignment> = result
                .suggestions
                .iter()
                .filter_map(|s| s.project_id.clone().map(|p| Assignment { doc_id: s.doc_id.clone(), project_id: p }))
                .collect();
            let applied = if apply.unwrap_or(false) && !assignments.is_empty() {
                let r = classify::apply(db, &assignments)?;
                out.documents.extend(assignments.iter().map(|a| a.doc_id.clone()));
                Some(r)
            } else {
                None
            };
            Ok(StepOutcome {
                summary: match (deps.ko(), applied.is_some()) {
                    (true, true) => format!(
                        "문서 {}건을 프로젝트에 배정(제안 {}건, 미정 {}건)",
                        assignments.len(),
                        result.suggested,
                        result.undecided
                    ),
                    (true, false) => {
                        format!("프로젝트 제안 {}건, 미정 {}건 — 적용하려면 '적용'", result.suggested, result.undecided)
                    }
                    (false, true) => format!(
                        "{} documents assigned ({} suggested, {} undecided)",
                        assignments.len(),
                        result.suggested,
                        result.undecided
                    ),
                    (false, false) => format!(
                        "{} suggestions, {} undecided — say 'apply' to assign",
                        result.suggested, result.undecided
                    ),
                },
                payload: json!({ "suggested": result.suggested, "undecided": result.undecided, "applied": applied.as_ref().map(|_| assignments.len()).unwrap_or(0), "suggestions": result.suggestions, "apply_result": applied }),
                imported: None,
            })
        }
        Op::ManageSchema { action, schema, name } => {
            let rec = resolve_schema(db, schema)?.ok_or_else(|| CoreError::NotFound(format!("schema {schema}")))?;
            let ko = deps.ko();
            let (summary, after) = match action {
                SchemaAction::Enable | SchemaAction::Disable => {
                    let on = *action == SchemaAction::Enable;
                    db.with(|c| pipeline_repo::set_schema_enabled(c, &rec.id, on))?;
                    (
                        if ko {
                            format!("양식 '{}' {}", rec.name, if on { "사용" } else { "사용 안 함" })
                        } else {
                            format!("Schema '{}' {}", rec.name, if on { "enabled" } else { "disabled" })
                        },
                        db.with(|c| pipeline_repo::get_schema(c, &rec.id))?,
                    )
                }
                SchemaAction::SetDefault => {
                    db.with_tx(|tx| pipeline_repo::set_default_schema(tx, &rec.id))?;
                    (
                        if ko {
                            format!(
                                "{} {} 유형의 기본 양식으로 지정",
                                josa::eul_reul(&format!("'{}'", rec.name)),
                                rec.doc_type
                            )
                        } else {
                            format!("'{}' is now the default schema of {}", rec.name, rec.doc_type)
                        },
                        db.with(|c| pipeline_repo::get_schema(c, &rec.id))?,
                    )
                }
                SchemaAction::Delete => {
                    db.with_tx(|tx| pipeline_repo::delete_schema(tx, &rec.id))?;
                    (
                        if ko {
                            format!("양식 '{}' 삭제", rec.name)
                        } else {
                            format!("Schema '{}' deleted", rec.name)
                        },
                        None,
                    )
                }
                SchemaAction::Rename => {
                    let new_name = name
                        .clone()
                        .map(|n| n.trim().to_string())
                        .filter(|n| !n.is_empty())
                        .ok_or_else(|| CoreError::Invalid("name_required".into()))?;
                    let draft = SchemaDraft {
                        id: Some(rec.id.clone()),
                        name: new_name.clone(),
                        name_en: rec.name_en.clone(),
                        doc_type: rec.doc_type.clone(),
                        source: rec.source,
                        description_ko: rec.description_ko.clone(),
                        description_en: rec.description_en.clone(),
                        definition: rec.definition.clone(),
                        form_document_id: rec.form_document_id.clone(),
                        note: Some(if ko { "이름 변경".into() } else { "renamed".into() }),
                        sample_document_ids: Vec::new(),
                    };
                    let saved = db.with_tx(|tx| pipeline_repo::save_schema(tx, &draft))?;
                    (
                        if ko {
                            format!("양식 이름을 {} 바꿈", josa::euro_ro(&format!("'{}'", saved.name)))
                        } else {
                            format!("Schema renamed to '{}'", saved.name)
                        },
                        Some(saved),
                    )
                }
            };
            Ok(StepOutcome {
                summary,
                payload: json!({ "action": action, "schema": after, "schema_id": rec.id }),
                imported: None,
            })
        }
        // UI-side ops never reach here (`runs_in_ui`).
        _ => Ok(StepOutcome { summary: String::new(), payload: Value::Null, imported: None }),
    }
}

/// The outcome of an answer / explanation whose scope matches no document: "no evidence",
/// without asking the model and without widening the search to the whole library.
fn no_documents_answer(deps: &ExecDeps<'_>, question: &str, kind: AnswerKind) -> StepOutcome {
    let text = deps.t("범위에 해당하는 문서가 없습니다", "No document matches the scope").to_string();
    let answer = crate::knowledge::Answer {
        kind,
        question: question.to_string(),
        text: text.clone(),
        no_evidence: true,
        ..Default::default()
    };
    StepOutcome { summary: text, payload: serde_json::to_value(&answer).unwrap_or(Value::Null), imported: None }
}

/// The field of a document's latest extraction that `review correct` names: by path
/// (`supplier.brn`, `items[2].amount`), by key, or by label in either language — a scalar field
/// for a key / label, and a label the reference is only part of when exactly one field has it.
fn review_field<'a>(
    fields: &'a [crate::pipeline::FieldRecord],
    reference: &str,
) -> Option<&'a crate::pipeline::FieldRecord> {
    let reference = reference.trim();
    let key = compact(reference);
    if key.is_empty() {
        return None;
    }
    let scalar = || fields.iter().filter(|f| f.row.is_none());
    fields
        .iter()
        .find(|f| f.path == reference)
        .or_else(|| scalar().find(|f| compact(&f.key) == key || compact(&f.path) == key))
        .or_else(|| scalar().find(|f| compact(&f.label_ko) == key || compact(&f.label_en) == key))
        .or_else(|| {
            let mut partial =
                scalar().filter(|f| compact(&f.label_ko).contains(&key) || compact(&f.label_en).contains(&key));
            match (partial.next(), partial.next()) {
                (Some(f), None) => Some(f),
                _ => None,
            }
        })
}

/// Resolves the rule references of `check_consistency` to rule ids: a rule id, or words of a
/// rule's description ("계약금액 규칙만 검사해줘" → the rules that mention 계약금액). Empty = every
/// enabled rule. A reference that matches no enabled rule is an error — the check must not run
/// zero rules and store that as the project's latest result (audit PRJ-12).
fn resolve_rules(db: &Db, project_id: &str, references: &[String]) -> Result<Vec<String>> {
    if references.is_empty() {
        return Ok(Vec::new());
    }
    let rules = db.with(|c| projects_repo::rules_of(c, project_id))?;
    let mut out: Vec<String> = Vec::new();
    for reference in references {
        let reference = reference.trim();
        let words = reference.trim_end_matches("규칙").trim_end_matches("rules").trim_end_matches("rule").trim();
        let key = compact(words);
        let matched: Vec<&projects::Rule> = rules
            .iter()
            .filter(|r| r.id == reference || (key.chars().count() >= 2 && compact(&r.description).contains(&key)))
            .collect();
        if matched.is_empty() {
            return Err(CoreError::Invalid(format!("rule_unknown: {reference}")));
        }
        if !matched.iter().any(|r| r.enabled) {
            return Err(CoreError::Invalid(format!("rule_disabled: {reference}")));
        }
        for r in matched.into_iter().filter(|r| r.enabled) {
            if !out.contains(&r.id) {
                out.push(r.id.clone());
            }
        }
    }
    Ok(out)
}

fn non_empty(docs_: Vec<DocumentRecord>) -> Result<Vec<DocumentRecord>> {
    if docs_.is_empty() {
        return Err(CoreError::Invalid("no_documents".into()));
    }
    Ok(docs_)
}

/// Exactly one document in scope (`code` names the op's need otherwise).
fn one_document(docs_: Vec<DocumentRecord>, code: &str) -> Result<DocumentRecord> {
    match docs_.len() {
        0 => Err(CoreError::Invalid("no_documents".into())),
        1 => Ok(docs_.into_iter().next().expect("one")),
        _ => Err(CoreError::Invalid(code.into())),
    }
}

/// Pages that start a new document for `split`: the explicit boundaries, or every `every` pages.
fn split_boundaries(page_count: u32, boundaries: &[u32], every: Option<u32>) -> Result<Vec<u32>> {
    let pages = page_count.max(1);
    let mut bounds: Vec<u32> = boundaries.to_vec();
    if bounds.is_empty() {
        if let Some(n) = every.filter(|n| *n >= 1) {
            let mut p = n + 1;
            while p <= pages {
                bounds.push(p);
                p += n;
            }
        }
    }
    bounds.retain(|b| *b >= 2 && *b <= pages);
    bounds.sort_unstable();
    bounds.dedup();
    if bounds.is_empty() {
        return Err(CoreError::Invalid("boundaries_required".into()));
    }
    Ok(bounds)
}

fn role_label(role: DocumentRole, ko: bool) -> &'static str {
    match (role, ko) {
        (DocumentRole::Knowledge, true) => "참고 자료",
        (DocumentRole::Knowledge, false) => "reference material",
        (DocumentRole::Form, true) => "양식",
        (DocumentRole::Form, false) => "form",
        (DocumentRole::Generated, true) => "생성 문서",
        (DocumentRole::Generated, false) => "generated",
        (_, true) => "업무 문서",
        (_, false) => "work document",
    }
}

/// A reference group: a project's own (`project`), else a common group by id or name (`group`).
fn resolve_group(db: &Db, group: Option<&str>, project: Option<&str>) -> Result<Option<Collection>> {
    if let Some(pid) = project.map(str::trim).filter(|p| !p.is_empty()) {
        // Looking a project's group up writes nothing: the group is stored when it first receives a
        // document (`add_docs_to_collection`), not when a command merely names it (audit PRJ-16).
        return Ok(Some(db.with(|c| knowledge_repo::peek_project_reference_group(c, pid))?));
    }
    let Some(g) = group.map(str::trim).filter(|g| !g.is_empty()) else { return Ok(None) };
    if let Some(col) = db.with(|c| knowledge_repo::get_collection(c, g))? {
        return Ok(Some(col));
    }
    let want = compact(g);
    Ok(db.with(knowledge_repo::list_collections)?.into_iter().find(|c| c.is_common() && compact(&c.name) == want))
}

/// A schema by id, name (ko / en) or document-type slug (its default schema).
fn resolve_schema(db: &Db, key: &str) -> Result<Option<crate::pipeline::schema::SchemaRecord>> {
    if let Some(s) = db.with(|c| pipeline_repo::get_schema(c, key))? {
        return Ok(Some(s));
    }
    let want = compact(key);
    if want.is_empty() {
        return Ok(None);
    }
    let all = db.with(pipeline_repo::list_schemas)?;
    Ok(all
        .iter()
        .find(|s| compact(&s.name) == want || s.name_en.as_deref().is_some_and(|n| compact(n) == want))
        .or_else(|| all.iter().find(|s| compact(&s.doc_type) == want && s.is_default))
        .or_else(|| all.iter().find(|s| compact(&s.doc_type) == want))
        .cloned())
}

/// A project template by id, or by its (compact) Korean / English name as the planner hands it on.
fn resolve_template(db: &Db, key: &str) -> Result<Option<projects::ProjectTemplate>> {
    if let Some(t) = db.with(|c| projects_repo::get_template(c, key))? {
        return Ok(Some(t));
    }
    let want = super::compact(key);
    if want.is_empty() {
        return Ok(None);
    }
    let all = db.with(projects_repo::list_templates)?;
    Ok(all.into_iter().find(|t| super::compact(&t.name_ko) == want || super::compact(&t.name_en) == want))
}

/// Runs the pipeline up to `level` over `docs_` (the step's working set, see `pipeline_set`).
#[allow(clippy::too_many_arguments)]
fn pipeline_step(
    deps: &ExecDeps<'_>,
    docs_: Vec<DocumentRecord>,
    level: PipelineLevel,
    schema: SchemaChoice,
    force: bool,
    profile: Option<Profile>,
    pages: Vec<u32>,
    progress: &mut dyn FnMut(&str, u32, u32, &str),
    out: &mut PlanExecution,
) -> Result<StepOutcome> {
    if docs_.is_empty() {
        return Ok(nothing_in_scope(deps));
    }
    let chat = deps.chat.ok_or_else(|| no_model(deps))?;
    let model = deps.model.clone().ok_or_else(|| no_model(deps))?;
    let _lease = LeaseGuard::acquire(deps.lease);
    let pdeps = PipelineDeps {
        import: ImportDeps {
            db: deps.import.db,
            paths: deps.import.paths,
            render: deps.import.render,
            printer: deps.import.printer,
            render_options: deps.import.render_options.clone(),
        },
        chat,
        model,
        settings: deps.settings.clone(),
        profile: deps.profile,
        pii_masking: deps.pii_masking,
        cancel: deps.cancel,
        ctx: deps.ctx_size,
    };
    let req = ProcessRequest { document_ids: ids(&docs_), level, schema, force, profile, pages, ..Default::default() };
    let stage = level.as_str().to_string();
    let report = pipeline_run::process(&pdeps, &req, &mut |p| {
        // Page boundary: pause / let interactive requests in (a cancel surfaces through the flag).
        let _ = deps.checkpoint();
        progress(&stage, p.done, p.total, &p.current);
    })?;
    out.documents.extend(report.processed.iter().cloned());
    out.documents.extend(report.failed.iter().map(|(id, _)| id.clone()));
    if report.cancelled {
        return Err(CoreError::Cancelled);
    }
    let summary = if deps.ko() {
        match level {
            PipelineLevel::Classify => format!("{}건 분류", report.classified),
            PipelineLevel::Ocr => format!("OCR {}쪽", report.ocr_pages),
            PipelineLevel::Extract => format!(
                "{}건 추출 · 검토 대기 {}건{}",
                report.extracted,
                report.review_pending,
                if report.failed.is_empty() { String::new() } else { format!(" · 실패 {}건", report.failed.len()) }
            ),
        }
    } else {
        match level {
            PipelineLevel::Classify => format!("{} classified", report.classified),
            PipelineLevel::Ocr => format!("OCR {} pages", report.ocr_pages),
            PipelineLevel::Extract => format!(
                "{} extracted · {} need review{}",
                report.extracted,
                report.review_pending,
                if report.failed.is_empty() { String::new() } else { format!(" · {} failed", report.failed.len()) }
            ),
        }
    };
    if !report.failed.is_empty() && report.processed.is_empty() {
        return Err(CoreError::Invalid(
            report.failed.iter().map(|(id, e)| format!("{id}: {e}")).collect::<Vec<_>>().join("; "),
        ));
    }
    Ok(StepOutcome { summary, payload: serde_json::to_value(&report).unwrap_or(Value::Null), imported: None })
}

#[allow(clippy::too_many_arguments)]
fn organize_step(
    deps: &ExecDeps<'_>,
    plan: &Plan,
    imported: Option<&[String]>,
    name_template: Option<String>,
    folder_template: Option<String>,
    only_reviewed: bool,
    dry_run: bool,
    kind: &str,
    progress: &mut dyn FnMut(&str, u32, u32, &str),
    out: &mut PlanExecution,
) -> Result<StepOutcome> {
    let Some(scope_) = step_scope(plan, imported) else { return Ok(nothing_in_scope(deps)) };
    let req = OrganizeRequest { scope: scope_, name_template, folder_template, only_reviewed, language: deps.language };
    let oplan = organize::plan(deps.db(), deps.paths(), &req)?;
    if dry_run {
        return Ok(StepOutcome {
            summary: if deps.ko() {
                format!("미리보기: 이동 {}건 · 보류 {}건 · 변경 없음 {}건", oplan.moves, oplan.held, oplan.unchanged)
            } else {
                format!("Preview: {} moves · {} held · {} unchanged", oplan.moves, oplan.held, oplan.unchanged)
            },
            payload: serde_json::to_value(&oplan).unwrap_or(Value::Null),
            imported: None,
        });
    }
    let label = if deps.ko() {
        format!(
            "{} {}건",
            match kind {
                "rename" => "이름 변경",
                "move" => "이동",
                _ => "정리",
            },
            oplan.moves
        )
    } else {
        format!("{kind} {} files", oplan.moves)
    };
    let report = organize::apply(
        deps.db(),
        deps.paths(),
        &oplan,
        kind,
        &label,
        "planner",
        deps.cancel,
        &mut |done, total, name| {
            let _ = deps.checkpoint();
            progress(kind, done as u32, total as u32, name);
        },
    )?;
    out.documents.extend(report.documents.iter().cloned());
    if let Some(b) = report.batch_id.clone() {
        out.batches.push(b);
    }
    if report.cancelled {
        return Err(CoreError::Cancelled);
    }
    Ok(StepOutcome {
        summary: if deps.ko() {
            format!("이동 {}건 · 보류 {}건 · 실패 {}건", report.moved, oplan.held, report.failed.len())
        } else {
            format!("{} moved · {} held · {} failed", report.moved, oplan.held, report.failed.len())
        },
        payload: json!({ "batch_id": report.batch_id, "moved": report.moved, "held": oplan.held, "failed": report.failed, "items": report.items }),
        imported: None,
    })
}

/// Previews the side effects of a plan for the confirmation card.
pub fn preview(
    db: &Db,
    paths: &WorkspacePaths,
    plan: &Plan,
    settings: &ProcessingSettings,
    language: Language,
) -> Result<PlanPreview> {
    let ko = language == Language::Ko;
    let docs_ = db.with(|c| scope::resolve(c, &plan_scope(plan)))?;
    let mut out = PlanPreview {
        document_count: docs_.len() as u64,
        document_ids: ids(&docs_),
        steps: Vec::new(),
        requires_confirmation: plan.requires_confirmation,
        uses_scope: super::validate::uses_scope(plan),
    };
    for (index, op) in plan.operations.iter().enumerate() {
        let kind = op.kind();
        let (summary, payload, confirm) = match op {
            Op::Organize { template, folder_template, group_by, only_reviewed, dry_run } => {
                let req = OrganizeRequest {
                    scope: plan.scope.clone(),
                    name_template: Some(template.clone().unwrap_or_else(|| organize::NAME_TEMPLATE.into())),
                    folder_template: Some(
                        folder_template
                            .clone()
                            .unwrap_or_else(|| organize::folder_template(group_by.unwrap_or(GroupBy::DocType)).into()),
                    ),
                    only_reviewed: only_reviewed.unwrap_or(settings.organize_reviewed_only),
                    language,
                };
                organize_preview(db, paths, &req, ko, !dry_run.unwrap_or(false))
            }
            Op::Rename { template, only_reviewed } => organize_preview(
                db,
                paths,
                &OrganizeRequest {
                    scope: plan.scope.clone(),
                    name_template: Some(template.clone()),
                    folder_template: None,
                    only_reviewed: only_reviewed.unwrap_or(settings.organize_reviewed_only),
                    language,
                },
                ko,
                true,
            ),
            Op::Move { folder, only_reviewed, .. } => organize_preview(
                db,
                paths,
                &OrganizeRequest {
                    scope: plan.scope.clone(),
                    name_template: None,
                    folder_template: Some(folder.clone()),
                    only_reviewed: only_reviewed.unwrap_or(settings.organize_reviewed_only),
                    language,
                },
                ko,
                true,
            ),
            Op::Export { format, path, columns, layout, include, target } => {
                let req = ExportRequest {
                    format: *format,
                    target: target.unwrap_or(ExportTarget::Fields),
                    path: path.clone(),
                    columns: columns.clone(),
                    layout: layout.unwrap_or(ExportLayout::SheetPerType),
                    include: include.clone(),
                    scope: plan.scope.clone(),
                    only_reviewed: false,
                    overwrite: false,
                    language,
                };
                match export::preview(db, paths, &req) {
                    Ok(p) => {
                        let s = if ko {
                            format!(
                                "{} → {}{}",
                                count_label_lang(p.documents as usize, ko),
                                p.path,
                                if p.exists { " (기존 파일 덮어씀)" } else { "" }
                            )
                        } else {
                            format!(
                                "{} → {}{}",
                                count_label_lang(p.documents as usize, ko),
                                p.path,
                                if p.exists { " (overwrites)" } else { "" }
                            )
                        };
                        let exists = p.exists;
                        (s, serde_json::to_value(&p).unwrap_or(Value::Null), exists)
                    }
                    Err(e) => (e.to_string(), Value::Null, false),
                }
            }
            Op::Extract { schema_ref, fields, force } => {
                let name = schema_ref
                    .as_deref()
                    .and_then(|id| db.with(|c| pipeline_repo::get_schema(c, id)).ok().flatten())
                    .map(|s| s.name)
                    .unwrap_or_else(|| if ko { "자동(유형별 스키마)".into() } else { "auto".into() });
                let head = if ko {
                    format!(
                        "스키마 {name} · {}",
                        if fields.is_empty() {
                            "전체 필드".to_string()
                        } else {
                            format!("필드 {}개", fields.len())
                        }
                    )
                } else {
                    format!(
                        "schema {name} · {}",
                        if fields.is_empty() { "all fields".to_string() } else { format!("{} fields", fields.len()) }
                    )
                };
                // What the step does to documents that already have an extraction (audit PLN-3):
                // kept unless `force`; a forced run that replaces reviewed results is confirmed.
                let force = force.unwrap_or(false);
                let after_import = plan.operations[..index].iter().any(|o| matches!(o, Op::Import { .. }));
                if after_import {
                    // The working set is the import's: its documents are not known before it runs.
                    let tail = match (ko, force) {
                        (true, false) => "가져온 문서 대상(이미 추출된 문서는 건너뜀)",
                        (true, true) => "가져온 문서를 다시 추출(기존 추출·검토 결과 대체)",
                        (false, false) => "the imported documents (already extracted ones are skipped)",
                        (false, true) => "re-extracts the imported documents (replaces existing results and reviews)",
                    };
                    (
                        format!("{head} · {tail}"),
                        json!({ "schema": name, "fields": fields, "force": force, "after_import": true }),
                        force,
                    )
                } else {
                    let extracted: Vec<String> =
                        docs_.iter().filter(|d| d.last_run_id.is_some()).map(|d| d.id.clone()).collect();
                    let reviewed = if force && !extracted.is_empty() {
                        db.with(|c| scope::reviewed_documents(c, &extracted))?.len()
                    } else {
                        0
                    };
                    let run = if force { docs_.len() } else { docs_.len() - extracted.len() };
                    let tail = match (ko, force) {
                        (true, false) if extracted.is_empty() => format!("추출 {run}건"),
                        (true, false) => format!("추출 {run}건 · 이미 추출된 {}건은 건너뜀", extracted.len()),
                        (true, true) if extracted.is_empty() => format!("추출 {run}건"),
                        (true, true) => format!(
                            "다시 추출 {run}건 — 기존 결과 {}건 대체{}",
                            extracted.len(),
                            if reviewed > 0 {
                                format!("(검토한 문서 {reviewed}건 포함)")
                            } else {
                                String::new()
                            }
                        ),
                        (false, false) if extracted.is_empty() => format!("{run} to extract"),
                        (false, false) => format!("{run} to extract · {} already extracted, skipped", extracted.len()),
                        (false, true) if extracted.is_empty() => format!("{run} to extract"),
                        (false, true) => format!(
                            "re-extracts {run} — replaces {} existing results{}",
                            extracted.len(),
                            if reviewed > 0 { format!(" ({reviewed} reviewed)") } else { String::new() }
                        ),
                    };
                    (
                        format!("{head} · {tail}"),
                        json!({
                            "schema": name,
                            "fields": fields,
                            "force": force,
                            "documents": docs_.len(),
                            "extract": run,
                            "already_extracted": extracted.len(),
                            "replace": if force { extracted.len() } else { 0 },
                            "reviewed": reviewed,
                        }),
                        force && reviewed > 0,
                    )
                }
            }
            Op::Delete { .. } => (
                if ko {
                    format!("{} 휴지통으로", count_label_lang(docs_.len(), ko))
                } else {
                    format!("{} to the recycle bin", count_label_lang(docs_.len(), ko))
                },
                json!({ "documents": docs_.len() }),
                true,
            ),
            Op::Undo { target } => {
                let batch = match target.as_deref() {
                    Some(id) => db.with(|c| journal::get_batch(c, id))?,
                    None => db.with(journal::last_undoable)?,
                };
                match batch {
                    Some(b) => (
                        if ko {
                            format!("되돌리기: {} ({}건)", b.label.clone().unwrap_or_else(|| b.kind.clone()), b.entries)
                        } else {
                            format!(
                                "Undo: {} ({} entries)",
                                b.label.clone().unwrap_or_else(|| b.kind.clone()),
                                b.entries
                            )
                        },
                        serde_json::to_value(&b).unwrap_or(Value::Null),
                        true,
                    ),
                    None => (
                        if ko { "되돌릴 작업이 없습니다".into() } else { "Nothing to undo".into() },
                        Value::Null,
                        false,
                    ),
                }
            }
            Op::Redo {} => match db.with(journal::last_redoable)? {
                Some(b) => (
                    if ko {
                        format!("다시 실행: {}건", b.entries)
                    } else {
                        format!("Redo: {} entries", b.entries)
                    },
                    serde_json::to_value(&b).unwrap_or(Value::Null),
                    true,
                ),
                None => (
                    if ko { "다시 실행할 작업이 없습니다".into() } else { "Nothing to redo".into() },
                    Value::Null,
                    false,
                ),
            },
            Op::Classify { .. } | Op::Ocr { .. } | Op::Validate {} | Op::Summarize { .. } => {
                (count_label_lang(docs_.len(), ko), json!({ "documents": docs_.len() }), false)
            }
            // Bulk edits of several documents are confirmed first.
            Op::Tag { .. }
            | Op::SetDocType { .. }
            | Op::Review { action: ReviewOp::Approve | ReviewOp::Reject, .. } => {
                (count_label_lang(docs_.len(), ko), json!({ "documents": docs_.len() }), docs_.len() > 1)
            }
            Op::Import { source, path, paths: file_paths, recursive, .. } => match source {
                super::ImportFrom::Files => {
                    let existing = file_paths.iter().filter(|p| std::path::Path::new(p).is_file()).count();
                    (
                        if ko {
                            format!("파일 {}개 가져오기(찾은 파일 {existing}개)", file_paths.len())
                        } else {
                            format!("Import {} files ({existing} found)", file_paths.len())
                        },
                        json!({ "paths": file_paths, "existing": existing }),
                        true,
                    )
                }
                super::ImportFrom::Folder => {
                    let p = path.clone().unwrap_or_default();
                    let root = std::path::PathBuf::from(&p);
                    let root = if root.is_absolute() { root } else { paths.root.join(&root) };
                    let (files, truncated) = import::collect_paths(std::slice::from_ref(&root), 10_000);
                    let recurse = recursive.unwrap_or(false);
                    let n = if recurse {
                        files.len()
                    } else {
                        files.iter().filter(|f| f.parent() == Some(root.as_path())).count()
                    };
                    let more = if truncated { "+" } else { "" };
                    (
                        if ko {
                            format!("폴더 {p} · 파일 {n}{more}개를 워크스페이스로 복사")
                        } else {
                            format!("folder {p} · {n}{more} files copied into the workspace")
                        },
                        json!({ "folder": p, "files": n, "truncated": truncated, "recursive": recurse }),
                        true,
                    )
                }
                _ => {
                    let n = import::inbox_plan(db, paths, &ImportOptions::default())
                        .map(|p| p.importable().count())
                        .unwrap_or(0);
                    (if ko { format!("신규 {n}개") } else { format!("{n} new files") }, json!({ "new": n }), false)
                }
            },
            Op::DraftDocument { form_ref, sources, output, .. } => (
                if ko {
                    format!(
                        "{} 초안을 모델이 생성합니다(출처 {}건) → 문서 생성 화면에서 확인·저장",
                        form_ref.as_deref().unwrap_or("문서"),
                        sources.len()
                    )
                } else {
                    format!(
                        "The model drafts a {} ({} sources) → review and save in the document generator",
                        form_ref.as_deref().unwrap_or("document"),
                        sources.len()
                    )
                },
                json!({ "form_ref": form_ref, "sources": sources, "output": output }),
                true,
            ),
            Op::ReviseDraft { scope, section, instruction, .. } => (
                {
                    let what = match scope.as_str() {
                        "section" => {
                            if ko {
                                format!("{}절을", section.unwrap_or(1))
                            } else {
                                format!("section {}", section.unwrap_or(1))
                            }
                        }
                        "insert_after" => {
                            if ko {
                                "새 절을".into()
                            } else {
                                "a new section".into()
                            }
                        }
                        _ => {
                            if ko {
                                "전체를".into()
                            } else {
                                "the whole draft".into()
                            }
                        }
                    };
                    if ko {
                        format!(
                            "열린 초안의 {what} 모델이 고쳐 씁니다: {} → 문서 생성 화면에서 확인·저장",
                            crate::pipeline::prompts::truncate(instruction, 80)
                        )
                    } else {
                        format!("The model revises {what} of the open draft: {} → review and save in the document generator", crate::pipeline::prompts::truncate(instruction, 80))
                    }
                },
                json!({ "scope": scope, "section": section }),
                false,
            ),
            Op::DefineRule { project, template, description } => {
                // Deterministic preview: the phrase rules only (no model), so the card can show the grid.
                let pdeps = ProjectDeps { db, vlm: None, language };
                let draft = projects::define_rule(&pdeps, project.as_deref(), template.as_deref(), description).ok();
                (
                    if ko {
                        "규칙 JSON으로 변환한 뒤 규칙 그리드에서 확인합니다".into()
                    } else {
                        "Converted to rule JSON, then confirmed in the rule grid".into()
                    },
                    json!({ "description": description, "draft": draft }),
                    true,
                )
            }
            Op::RequestSupplement { output, .. } => (
                if ko {
                    "마지막 검사의 불일치·누락으로 보완요청서 초안을 만듭니다".into()
                } else {
                    "Drafts a supplement request from the last check's findings".into()
                },
                json!({ "output": output }),
                true,
            ),
            Op::SetRetention { item, days } => (
                match (ko, *days) {
                    (true, -1) => format!("{} 보존 기간을 무기한으로 바꿉니다", item.as_str()),
                    (true, d) => {
                        format!("{} 보존 기간을 {d}일로 바꿉니다(지난 항목은 다음 파기 작업에서 삭제)", item.as_str())
                    }
                    (false, -1) => format!("Keep {} forever", item.as_str()),
                    (false, d) => format!("Keep {} for {d} days (older items go at the next purge)", item.as_str()),
                },
                json!({ "item": item, "days": days }),
                true,
            ),
            Op::RegisterForm { doc_ref, name, doc_ids, .. } => (
                if ko {
                    if doc_ids.len() >= 2 {
                        format!("문서 {}개에서 필드를 도출·병합해 양식으로 등록합니다(필드 그리드 확인)", doc_ids.len())
                    } else {
                        "문서에서 필드를 도출해 양식으로 등록합니다(필드 그리드 확인)".into()
                    }
                } else if doc_ids.len() >= 2 {
                    format!(
                        "Derives and merges the fields of {} documents and registers the form (field grid)",
                        doc_ids.len()
                    )
                } else {
                    "Derives the fields and registers the form (field grid)".into()
                },
                json!({ "doc_ref": doc_ref, "name": name, "doc_ids": doc_ids }),
                true,
            ),
            // -- 0.6.0 confirmation previews -----------------------------------------------------
            Op::Split { boundaries, every } => {
                let bounds = docs_
                    .first()
                    .and_then(|d| split_boundaries(d.page_count, boundaries, *every).ok())
                    .unwrap_or_default();
                (
                    if ko {
                        format!(
                            "{}개 문서로 나눕니다(쪽 {})",
                            bounds.len() + 1,
                            bounds.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(", ")
                        )
                    } else {
                        format!(
                            "Split into {} documents (at pages {})",
                            bounds.len() + 1,
                            bounds.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(", ")
                        )
                    },
                    json!({ "documents": docs_.len(), "boundaries": bounds, "parts": bounds.len() + 1 }),
                    true,
                )
            }
            Op::Merge { name } => (
                if ko {
                    format!(
                        "{} → PDF 1개{}",
                        count_label_lang(docs_.len(), ko),
                        name.as_deref().map(|n| format!(" '{n}'")).unwrap_or_default()
                    )
                } else {
                    format!(
                        "{} → one PDF{}",
                        count_label_lang(docs_.len(), ko),
                        name.as_deref().map(|n| format!(" '{n}'")).unwrap_or_default()
                    )
                },
                json!({ "documents": docs_.len(), "name": name }),
                true,
            ),
            Op::EditText { find, replace, pages } => {
                let mut matches = 0usize;
                let mut touched = 0usize;
                for d in &docs_ {
                    let rows = db.with(|c| docs::list_pages(c, &d.id, true)).unwrap_or_default();
                    for p in rows.iter().filter(|p| pages.is_empty() || pages.contains(&p.page_no)) {
                        let n = p.text.as_deref().map(|t| t.matches(find.as_str()).count()).unwrap_or(0);
                        if n > 0 {
                            matches += n;
                            touched += 1;
                        }
                    }
                }
                (
                    if ko {
                        format!("'{find}' {matches}곳({touched}쪽) → '{replace}'")
                    } else {
                        format!("{matches} matches of '{find}' on {touched} pages → '{replace}'")
                    },
                    json!({ "documents": docs_.len(), "matches": matches, "pages": touched }),
                    true,
                )
            }
            Op::DeleteProject { project } => {
                let (name, members) = project
                    .as_deref()
                    .and_then(|pid| {
                        db.with(|c| projects_repo::get_project(c, pid))
                            .ok()
                            .flatten()
                            .map(|p| (p.name, p.document_count))
                    })
                    .unwrap_or_default();
                (
                    if ko {
                        format!("프로젝트 '{name}' 삭제 — 문서 {members}건은 문서함에 남음")
                    } else {
                        format!("Delete project '{name}' — its {members} documents stay in the library")
                    },
                    json!({ "project_id": project, "name": name, "documents": members }),
                    true,
                )
            }
            Op::ManageReference { action: ReferenceAction::Delete, group, project, .. } => {
                let col = resolve_group(db, group.as_deref(), project.as_deref()).ok().flatten();
                let (name, n) = col.map(|c| (c.name, c.doc_count)).unwrap_or_default();
                (
                    if ko {
                        format!("참고 자료 묶음 '{name}' 삭제 — 문서 {n}건은 남음")
                    } else {
                        format!("Delete reference group '{name}' — its {n} documents stay")
                    },
                    json!({ "group": name, "documents": n }),
                    true,
                )
            }
            Op::ManageTemplate { action: TemplateAction::Delete, template, name, .. } => {
                let key = template.clone().or_else(|| name.clone()).unwrap_or_default();
                let t = resolve_template(db, &key).ok().flatten();
                (
                    match (&t, ko) {
                        (Some(t), true) => format!("템플릿 '{}' 삭제", t.name_ko),
                        (Some(t), false) => format!("Delete template '{}'", t.name_en),
                        (None, true) => format!("템플릿 {} 찾을 수 없음", josa::eul_reul(&format!("'{key}'"))),
                        (None, false) => format!("Template '{key}' not found"),
                    },
                    json!({ "template": t.as_ref().map(|t| t.id.clone()), "builtin": t.as_ref().map(|t| t.builtin) }),
                    true,
                )
            }
            Op::ManageSchema { action: SchemaAction::Delete, schema, .. } => {
                let rec = resolve_schema(db, schema).ok().flatten();
                (
                    match (&rec, ko) {
                        (Some(r), true) => format!("양식 '{}' 삭제(사용 {}회)", r.name, r.use_count),
                        (Some(r), false) => format!("Delete schema '{}' (used {} times)", r.name, r.use_count),
                        (None, true) => format!("양식 {} 찾을 수 없음", josa::eul_reul(&format!("'{schema}'"))),
                        (None, false) => format!("Schema '{schema}' not found"),
                    },
                    json!({ "schema": rec.as_ref().map(|r| r.id.clone()), "builtin": rec.as_ref().map(|r| r.builtin_key.is_some()) }),
                    true,
                )
            }
            Op::ClassifyProjects { apply, use_model } => {
                // Deterministic suggestions (no model) so the card shows what would be assigned.
                let pdeps = ProjectDeps { db, vlm: None, language };
                let req = ClassifyRequest {
                    doc_ids: ids(&docs_),
                    project_ids: Vec::new(),
                    use_model: false,
                    include_assigned: false,
                };
                let result = classify::suggest(&pdeps, &req).ok();
                let (suggested, undecided) = result.as_ref().map(|r| (r.suggested, r.undecided)).unwrap_or_default();
                (
                    if ko {
                        format!(
                            "프로젝트 제안 {suggested}건 · 미정 {undecided}건{}",
                            if apply.unwrap_or(false) { " → 바로 배정" } else { "" }
                        )
                    } else {
                        format!(
                            "{suggested} suggestions, {undecided} undecided{}",
                            if apply.unwrap_or(false) { " → assigned" } else { "" }
                        )
                    },
                    json!({ "suggested": suggested, "undecided": undecided, "apply": apply, "use_model": use_model, "suggestions": result.map(|r| r.suggestions) }),
                    apply.unwrap_or(false),
                )
            }
            // A new project takes the documents the command names — the card says how many (audit PLN-5).
            Op::CreateProject { name, .. } => {
                let n = if plan.scope.is_empty() { 0 } else { docs_.len() };
                (
                    match (ko, n) {
                        (true, 0) => format!("새 프로젝트 '{name}'(문서 없이 만듦)"),
                        (true, n) => format!("새 프로젝트 '{name}' · 문서 {n}건을 함께 넣음"),
                        (false, 0) => format!("New project '{name}' (no documents)"),
                        (false, n) => format!("New project '{name}' with {n} documents"),
                    },
                    json!({ "name": name, "documents": n }),
                    true,
                )
            }
            // Taking documents out of a project deletes their copies in the project folder: two or
            // more are confirmed first (audit PLN-6).
            Op::RemoveFromProject { project, doc_ids } => {
                let n = if doc_ids.is_empty() { docs_.len() } else { doc_ids.len() };
                let name = project
                    .as_deref()
                    .and_then(|pid| db.with(|c| projects_repo::get_project(c, pid)).ok().flatten())
                    .map(|p| p.name)
                    .unwrap_or_default();
                (
                    if ko {
                        format!("프로젝트 '{name}'에서 문서 {n}건 제외(프로젝트 폴더의 사본 삭제, 원본은 남음)")
                    } else {
                        format!("{n} documents leave project '{name}' (their copies in the project folder are deleted; the originals stay)")
                    },
                    json!({ "project_id": project, "name": name, "documents": n }),
                    n >= 2,
                )
            }
            // Which rules a partial check runs (a reference that matches none fails the step — audit PRJ-12).
            Op::CheckConsistency { project, rules } if !rules.is_empty() => {
                let resolved = project.as_deref().map(|pid| resolve_rules(db, pid, rules));
                match resolved {
                    Some(Ok(ids_)) => (
                        if ko {
                            format!("규칙 {}개만 검사", ids_.len())
                        } else {
                            format!("checks {} rules only", ids_.len())
                        },
                        json!({ "rules": ids_ }),
                        false,
                    ),
                    Some(Err(e)) => (
                        if ko {
                            format!("검사할 규칙을 찾을 수 없음: {}", rules.join(", "))
                        } else {
                            format!("no such rule: {}", rules.join(", "))
                        },
                        json!({ "rules": [], "error": e.to_string() }),
                        false,
                    ),
                    None => (String::new(), Value::Null, false),
                }
            }
            // `review correct`: the field and its current value in each document.
            Op::Review { action: ReviewOp::Correct, field, value, .. } => {
                let reference = field.as_deref().unwrap_or_default();
                let mut matches = Vec::new();
                for d in docs_.iter().take(200) {
                    let fields = db.with(|c| pipeline_repo::latest_fields(c, &d.id)).unwrap_or_default();
                    if let Some(f) = review_field(&fields, reference) {
                        matches.push(json!({ "document_id": d.id, "field_id": f.id, "path": f.path, "label_ko": f.label_ko, "label_en": f.label_en, "value": f.value }));
                    }
                }
                let new = value.as_deref().unwrap_or_default();
                let label = matches
                    .first()
                    .and_then(|m| m.get(if ko { "label_ko" } else { "label_en" }).and_then(Value::as_str))
                    .filter(|l| !l.is_empty())
                    .unwrap_or(reference)
                    .to_string();
                let summary = match (matches.len(), ko) {
                    (0, true) => format!("문서 {}건에서 필드 '{reference}' 찾을 수 없음", docs_.len()),
                    (0, false) => format!("field '{reference}' not found in {} documents", docs_.len()),
                    (1, _) => format!(
                        "{label}: {} → {new}",
                        matches[0].get("value").and_then(Value::as_str).unwrap_or(if ko {
                            "(값 없음)"
                        } else {
                            "(empty)"
                        })
                    ),
                    (n, true) => format!("문서 {n}건의 '{label}' → {new}"),
                    (n, false) => format!("'{label}' of {n} documents → {new}"),
                };
                (
                    summary,
                    json!({ "documents": docs_.len(), "field": reference, "value": new, "matches": matches }),
                    true,
                )
            }
            // The folder a new workspace is created in is confirmed (audit PLN-18).
            Op::Workspace { action: super::WorkspaceAction::Create, path } => {
                let p = path.clone().unwrap_or_default();
                (
                    if ko {
                        format!("새 워크스페이스 폴더: {p}")
                    } else {
                        format!("New workspace folder: {p}")
                    },
                    json!({ "path": p }),
                    true,
                )
            }
            _ => (String::new(), Value::Null, op.needs_confirmation()),
        };
        if confirm {
            out.requires_confirmation = true;
        }
        out.steps.push(StepPreview { index, op: kind, summary, payload, confirm });
    }
    Ok(out)
}

fn count_label_lang(n: usize, ko: bool) -> String {
    if ko {
        format!("문서 {n}건")
    } else {
        format!("{n} document{}", if n == 1 { "" } else { "s" })
    }
}

fn organize_preview(
    db: &Db,
    paths: &WorkspacePaths,
    req: &OrganizeRequest,
    ko: bool,
    confirm: bool,
) -> (String, Value, bool) {
    match organize::plan(db, paths, req) {
        Ok(p) => {
            let s = if ko {
                format!(
                    "이동 {}건 · 보류 {}건 · 변경 없음 {}건{}",
                    p.moves,
                    p.held,
                    p.unchanged,
                    if p.needs_review > 0 { format!(" · 값 미상 {}건", p.needs_review) } else { String::new() }
                )
            } else {
                format!(
                    "{} moves · {} held · {} unchanged{}",
                    p.moves,
                    p.held,
                    p.unchanged,
                    if p.needs_review > 0 {
                        format!(" · {} with unknown values", p.needs_review)
                    } else {
                        String::new()
                    }
                )
            };
            let mut preview = p.clone();
            preview.items.truncate(200);
            (s, serde_json::to_value(&preview).unwrap_or(Value::Null), confirm && p.moves > 0)
        }
        Err(e) => (e.to_string(), Value::Null, false),
    }
}
