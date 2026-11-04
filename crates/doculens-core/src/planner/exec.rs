//! Deterministic plan executor (§3.5 "결정적 실행기"): replays a validated
//! plan step by step over the documents in scope, reusing the pipeline,
//! knowledge, organise, aggregate and export modules. UI-side ops (settings,
//! navigation, job / engine control) are returned as `ui_actions`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::aggregate::{self, AggregateRequest};
use crate::db::Db;
use crate::documents::import::{self, ImportDeps, ImportOptions};
use crate::documents::{repo as docs, DocumentRecord};
use crate::error::{CoreError, Result};
use crate::export::{self, ExportRequest};
use crate::knowledge::answer::{self, AnswerDeps, ChatBackend};
use crate::knowledge::embed::Embedder;
use crate::knowledge::search::{self, SearchDeps};
use crate::knowledge::tokenize::Analyzer;
use crate::knowledge::{AnswerKind, AnswerRequest, SearchRequest, SearchScope, SummaryMode};
use crate::organize::{self, journal, OrganizeRequest};
use crate::paths::WorkspacePaths;
use crate::pipeline::repo::{self as pipeline_repo, ReviewFilter};
use crate::pipeline::review::{self, ReviewAction};
use crate::pipeline::run::{self as pipeline_run, PipelineDeps};
use crate::pipeline::{PipelineLevel, ProcessRequest, ReviewReason, SchemaChoice};
use crate::settings::{KnowledgeSettings, Language, ProcessingSettings, Profile};

use super::{
    scope, AggregateFn, ExportLayout, ExportTarget, GroupBy, OcrProfile, Op, OpKind, Plan, ReviewOnly, ReviewOp,
    SummarizeTarget, SummaryLength,
};

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

/// Documents the plan works on right now: the scope, narrowed to what an
/// earlier import step registered.
fn working_set(deps: &ExecDeps<'_>, plan: &Plan, imported: Option<&[String]>) -> Result<Vec<DocumentRecord>> {
    let mut docs = deps.db().with(|c| scope::resolve(c, &plan.scope))?;
    if let Some(ids) = imported {
        docs.retain(|d| ids.contains(&d.id));
    }
    Ok(docs)
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
        Op::Import { source, path, recursive, auto_pipeline: _ } => {
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
                    let files = if recursive.unwrap_or(true) {
                        files
                    } else {
                        files.into_iter().filter(|f| f.parent() == Some(root.as_path())).collect()
                    };
                    import::plan(db, deps.paths(), &files, &options, import::ImportSource::Folder)?
                }
                super::ImportFrom::Files => return Err(CoreError::Invalid("import_files_gui".into())),
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
            let mut all = report.imported.clone();
            all.extend(report.relinked.iter().cloned());
            // Documents already registered from the inbox stay in scope too.
            let existing = db.with(|c| scope::resolve(c, &plan.scope))?;
            all.extend(existing.iter().map(|d| d.id.clone()));
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
                payload: json!({ "imported": report.imported, "relinked": report.relinked, "failed": report.failed, "skipped": report.skipped }),
                imported: Some(all),
            })
        }
        Op::Classify { doc_types: _, force } => pipeline_step(
            deps,
            plan,
            imported,
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
            plan,
            imported,
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
            pipeline_step(
                deps,
                plan,
                imported,
                PipelineLevel::Extract,
                choice,
                force.unwrap_or(false),
                None,
                Vec::new(),
                progress,
                out,
            )
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
        Op::Review { action, only, field: _, value: _ } => {
            let docs_ = working_set(deps, plan, imported)?;
            let reason = match only {
                Some(ReviewOnly::LowConfidence) => Some(ReviewReason::LowConfidence),
                Some(ReviewOnly::Failed) => Some(ReviewReason::ValidationFailed),
                _ => None,
            };
            let mut changed = 0usize;
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
                    ReviewOp::Correct | ReviewOp::Show => {}
                }
                out.documents.push(d.id.clone());
            }
            Ok(StepOutcome {
                summary: if deps.ko() {
                    format!("필드 {changed}개 {}", if *action == ReviewOp::Approve { "승인" } else { "거부" })
                } else {
                    format!("{changed} fields {}", if *action == ReviewOp::Approve { "approved" } else { "rejected" })
                },
                payload: json!({ "changed": changed, "documents": ids(&docs_) }),
                imported: None,
            })
        }
        Op::Search { query, mode, top_k } => {
            let docs_ = if plan.scope.is_empty() { Vec::new() } else { working_set(deps, plan, imported)? };
            let mut req = SearchRequest::new(query);
            if let Some(m) = mode {
                req.mode = *m;
            }
            req.limit = top_k.unwrap_or(20).clamp(1, 200);
            req.scope = SearchScope {
                documents: ids(&docs_),
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
        Op::Answer { question, cite: _ } => {
            let chat = deps.chat.ok_or_else(|| no_model(deps))?;
            let docs_ = if plan.scope.is_empty() { Vec::new() } else { working_set(deps, plan, imported)? };
            let mut req = AnswerRequest::question(question);
            req.scope = SearchScope {
                documents: ids(&docs_),
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
            };
            let ans = answer::answer(&adeps, &req, &mut |_| !deps.cancelled())?;
            Ok(StepOutcome {
                summary: if ans.no_evidence {
                    deps.t("근거를 찾지 못했습니다", "No evidence found").into()
                } else {
                    crate::pipeline::prompts::truncate(&ans.text, 200)
                },
                payload: serde_json::to_value(&ans).unwrap_or(Value::Null),
                imported: None,
            })
        }
        Op::Summarize { target, length } => {
            let chat = deps.chat.ok_or_else(|| no_model(deps))?;
            let docs_ = working_set(deps, plan, imported)?;
            if docs_.is_empty() {
                return Err(CoreError::Invalid("no_documents".into()));
            }
            let mode = match length {
                Some(SummaryLength::Short) => SummaryMode::OneLine,
                Some(SummaryLength::Long) => SummaryMode::OnePage,
                None => SummaryMode::KeyPoints,
            };
            let selected: Vec<&DocumentRecord> = match target {
                Some(SummarizeTarget::Set) => docs_.iter().collect(),
                _ => docs_.iter().collect(),
            };
            let cancelled = || deps.cancelled();
            let adeps = AnswerDeps {
                search: SearchDeps { db, analyzer: deps.analyzer, embedder: deps.embedder },
                chat,
                model: deps.model.clone(),
                cancelled: Some(&cancelled),
            };
            let mut summaries = Vec::new();
            for (i, d) in selected.iter().enumerate() {
                deps.checkpoint()?;
                progress("summarize", i as u32, selected.len() as u32, &d.original_name);
                let mut req = AnswerRequest::question("");
                req.kind = AnswerKind::Summary;
                req.document_id = Some(d.id.clone());
                req.summary_mode = mode;
                req.context_tokens = deps.knowledge.context_tokens;
                let ans = answer::summarize(&adeps, &req, &mut |_| !deps.cancelled())?;
                summaries.push(json!({ "document_id": d.id, "name": d.original_name, "answer": ans }));
            }
            Ok(StepOutcome {
                summary: if deps.ko() {
                    format!("{} 요약", count_label(deps, selected.len()))
                } else {
                    format!("Summarised {}", count_label(deps, selected.len()))
                },
                payload: json!({ "summaries": summaries }),
                imported: None,
            })
        }
        Op::Aggregate { func, field, group_by, include_unreviewed } => {
            let req = AggregateRequest {
                func: *func,
                field: field.clone(),
                group_by: group_by.unwrap_or(GroupBy::None),
                include_unreviewed: include_unreviewed.unwrap_or(false),
                scope: plan.scope.clone(),
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
            let req = ExportRequest {
                format: *format,
                target: target.unwrap_or(ExportTarget::Fields),
                path: path.clone(),
                columns: columns.clone(),
                layout: layout.unwrap_or(ExportLayout::SheetPerType),
                include: include.clone(),
                scope: match imported {
                    Some(ids_) => super::Scope { doc_ids: ids_.to_vec(), ..plan.scope.clone() },
                    None => plan.scope.clone(),
                },
                only_reviewed: false,
                overwrite: true,
                language: deps.language,
            };
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
        // UI-side ops never reach here (`runs_in_ui`).
        _ => Ok(StepOutcome { summary: String::new(), payload: Value::Null, imported: None }),
    }
}

#[allow(clippy::too_many_arguments)]
fn pipeline_step(
    deps: &ExecDeps<'_>,
    plan: &Plan,
    imported: Option<&[String]>,
    level: PipelineLevel,
    schema: SchemaChoice,
    force: bool,
    profile: Option<Profile>,
    pages: Vec<u32>,
    progress: &mut dyn FnMut(&str, u32, u32, &str),
    out: &mut PlanExecution,
) -> Result<StepOutcome> {
    let chat = deps.chat.ok_or_else(|| no_model(deps))?;
    let model = deps.model.clone().ok_or_else(|| no_model(deps))?;
    let docs_ = working_set(deps, plan, imported)?;
    if docs_.is_empty() {
        return Ok(StepOutcome {
            summary: deps.t("대상 문서 없음", "No documents in scope").into(),
            payload: json!({ "documents": [] }),
            imported: None,
        });
    }
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
    let req = ProcessRequest { document_ids: ids(&docs_), level, schema, force, profile, pages, precise: None };
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
    let scope_ = match imported {
        Some(ids_) => super::Scope { doc_ids: ids_.to_vec(), ..plan.scope.clone() },
        None => plan.scope.clone(),
    };
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
    let docs_ = db.with(|c| scope::resolve(c, &plan.scope))?;
    let mut out = PlanPreview {
        document_count: docs_.len() as u64,
        document_ids: ids(&docs_),
        steps: Vec::new(),
        requires_confirmation: plan.requires_confirmation,
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
            Op::Extract { schema_ref, fields, .. } => {
                let name = schema_ref
                    .as_deref()
                    .and_then(|id| db.with(|c| pipeline_repo::get_schema(c, id)).ok().flatten())
                    .map(|s| s.name)
                    .unwrap_or_else(|| if ko { "자동(유형별 스키마)".into() } else { "auto".into() });
                (
                    if ko {
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
                            if fields.is_empty() {
                                "all fields".to_string()
                            } else {
                                format!("{} fields", fields.len())
                            }
                        )
                    },
                    json!({ "schema": name, "fields": fields }),
                    false,
                )
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
            Op::Classify { .. }
            | Op::Ocr { .. }
            | Op::Validate {}
            | Op::Summarize { .. }
            | Op::Tag { .. }
            | Op::SetDocType { .. } => (count_label_lang(docs_.len(), ko), json!({ "documents": docs_.len() }), false),
            Op::Import { source, .. } => {
                let n = match source {
                    super::ImportFrom::Inbox => import::inbox_plan(db, paths, &ImportOptions::default())
                        .map(|p| p.importable().count())
                        .unwrap_or(0),
                    _ => 0,
                };
                (if ko { format!("신규 {n}개") } else { format!("{n} new files") }, json!({ "new": n }), false)
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
