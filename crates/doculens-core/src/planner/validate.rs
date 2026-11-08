//! Programmatic plan validation (§3.5 "검증기"): serde parsing against the
//! typed ops (unknown parameters are rejected), op count, ISO dates, paths
//! inside the workspace, references that must exist (types, schemas, fields,
//! settings keys), and the normalisation the executor relies on (date
//! substitution, slug resolution, recomputed confirmation / intent).

use std::collections::BTreeSet;

use chrono::NaiveDate;
use serde_json::Value;

use crate::documents::DocumentStatus;
use crate::organize::template;
use crate::prerouter::{DateRange, Screen};

use super::{
    compact, AggregateFn, ExportFormat, ImportFrom, Op, OpKind, OpenTarget, Plan, PlanContext, PlanIssue, ReviewOp,
    SettingKey, MAX_OPS,
};

/// Ops that change documents must name their targets: an empty scope would mean the whole
/// library. Callers skip this when the sentence explicitly said so ("전부", "all documents").
pub fn scope_required(plan: &Plan) -> Option<PlanIssue> {
    if !plan.scope.is_empty() {
        return None;
    }
    let needs = plan.operations.iter().any(|op| match op {
        Op::SetDocType { .. } | Op::Tag { .. } | Op::Delete { .. } | Op::Rename { .. } | Op::Move { .. } => true,
        Op::Organize { dry_run, .. } => !dry_run.unwrap_or(false),
        Op::Review { action: ReviewOp::Approve | ReviewOp::Reject | ReviewOp::Correct, .. } => true,
        _ => false,
    });
    needs.then(|| {
        PlanIssue::error(
            "scope",
            "scope_required",
            "this plan changes documents but names none: select documents or give a folder / type / date range",
        )
    })
}

/// Parses the model's JSON into a plan. Serde errors become a single blocking
/// issue whose message names the offending field (fed back to the model).
pub fn parse_plan(raw: &Value) -> Result<Plan, PlanIssue> {
    serde_json::from_value::<Plan>(raw.clone()).map_err(|e| PlanIssue::error("", "schema", e.to_string()))
}

/// A workspace-relative path: forward slashes, no drive / root / `..` /
/// `.doculens`, ≤ 240 characters. Returns the normalised form.
pub fn workspace_relative(path: &str) -> Option<String> {
    let p = path.trim().replace('\\', "/");
    if p.is_empty() || p.starts_with('/') || p.starts_with("//") {
        return None;
    }
    if p.chars().nth(1) == Some(':') {
        return None; // drive letter
    }
    let mut parts = Vec::new();
    for c in p.split('/') {
        let c = c.trim();
        if c.is_empty() || c == "." {
            continue;
        }
        if c == ".." {
            return None;
        }
        parts.push(c);
    }
    if parts.is_empty() || parts[0].eq_ignore_ascii_case(".doculens") {
        return None;
    }
    let out = parts.join("/");
    if out.chars().count() > template::MAX_PATH_CHARS {
        return None;
    }
    Some(out)
}

fn iso_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

/// Validates and normalises a plan in place. Returns every finding; blocking
/// ones mean the plan must not run (`super::has_errors`).
pub fn validate(plan: &mut Plan, ctx: &PlanContext, date: Option<&DateRange>) -> Vec<PlanIssue> {
    let mut issues = Vec::new();
    if plan.confidence.is_nan() {
        plan.confidence = 0.0;
    }
    // A percentage (86) is tolerated; anything beyond that is not a confidence at all.
    if plan.confidence > 1.0 {
        plan.confidence = if plan.confidence <= 100.0 { plan.confidence / 100.0 } else { 0.0 };
    }
    plan.confidence = plan.confidence.clamp(0.0, 1.0);
    if let Some(s) = plan.summary.as_mut() {
        *s = s.trim().chars().take(300).collect();
        if s.is_empty() {
            plan.summary = None;
        }
    }

    // -- scope ----------------------------------------------------------------
    let f = &mut plan.scope.filter;
    if let Some(folder) = plan.scope.folder.take() {
        match workspace_relative(&folder) {
            Some(p) => plan.scope.folder = Some(p),
            None => issues.push(PlanIssue::error(
                "scope.folder",
                "path_invalid",
                format!("'{folder}' is not a workspace-relative folder"),
            )),
        }
    }
    if let Some(dt) = f.doc_type.clone() {
        match ctx.doc_type(&dt) {
            Some(d) => f.doc_type = Some(d.slug.clone()),
            None if ctx.doc_types.is_empty() => {}
            None => issues.push(PlanIssue::error(
                "scope.filter.doc_type",
                "doc_type_unknown",
                format!("unknown document type '{dt}'; known: {}", known_types(ctx)),
            )),
        }
    }
    for (path, value) in [("scope.filter.date_from", f.date_from.clone()), ("scope.filter.date_to", f.date_to.clone())]
    {
        if let Some(v) = value {
            if iso_date(&v).is_none() {
                issues.push(PlanIssue::error(
                    path,
                    "date_format",
                    format!("'{v}' is not an ISO-8601 date (YYYY-MM-DD)"),
                ));
            }
        }
    }
    if let (Some(a), Some(b)) = (f.date_from.as_deref().and_then(iso_date), f.date_to.as_deref().and_then(iso_date)) {
        if b < a {
            issues.push(PlanIssue::error("scope.filter.date_to", "date_order", "date_to must not precede date_from"));
        }
    }
    // Relative-date phrase resolved by the pre-router: deterministic override of a missing range.
    if let Some(d) = date {
        if f.date_from.is_none() && f.date_to.is_none() {
            f.date_from = Some(d.from.to_string());
            f.date_to = Some(d.to.to_string());
        }
    }
    if let Some(st) = f.status.as_deref() {
        if DocumentStatus::parse(st).is_none() {
            issues.push(PlanIssue::error("scope.filter.status", "status_unknown", format!("unknown status '{st}'")));
        }
    }
    if let Some(c) = f.min_confidence {
        if !(0.0..=1.0).contains(&c) {
            issues.push(PlanIssue::error(
                "scope.filter.min_confidence",
                "range",
                "min_confidence must be between 0 and 1",
            ));
        }
    }
    f.tags.retain(|t| !t.trim().is_empty());
    plan.scope.doc_ids.retain(|d| !d.trim().is_empty());
    plan.scope.doc_ids.dedup();

    // -- operations -----------------------------------------------------------
    if plan.operations.is_empty() {
        if plan.clarification_question.is_some() {
            let q = plan.clarification_question.clone().unwrap_or_default();
            plan.operations.push(Op::Clarify { question: q });
        } else {
            issues.push(PlanIssue::error("operations", "ops_empty", "the plan has no operations"));
        }
    }
    if plan.operations.len() > MAX_OPS {
        issues.push(PlanIssue::error("operations", "ops_many", format!("more than {MAX_OPS} operations")));
        plan.operations.truncate(MAX_OPS);
    }
    let clarifies = plan.operations.iter().filter(|o| matches!(o, Op::Clarify { .. })).count();
    if clarifies > 0 && plan.operations.len() > 1 {
        issues.push(PlanIssue::error("operations", "clarify_mixed", "clarify must be the only operation"));
    }
    let scope_doc_type = plan.scope.filter.doc_type.clone();
    let plan_scope_empty = plan.scope.is_empty();
    for (i, op) in plan.operations.iter_mut().enumerate() {
        let at = |field: &str| format!("operations[{i}].{field}");
        match op {
            Op::Import { source, path, .. } => {
                if *source == ImportFrom::Files {
                    issues.push(PlanIssue::error(
                        at("source"),
                        "import_files_gui",
                        "file selection needs the import dialog; use source=inbox or folder",
                    ));
                }
                if *source == ImportFrom::Folder && path.as_deref().map(str::trim).unwrap_or("").is_empty() {
                    issues.push(PlanIssue::error(at("path"), "path_required", "source=folder needs a path"));
                }
                if let Some(p) = path.as_mut() {
                    *p = p.trim().chars().take(1_000).collect();
                }
            }
            Op::Classify { doc_types, .. } => {
                let mut resolved = Vec::new();
                for t in doc_types.iter() {
                    match ctx.doc_type(t) {
                        Some(d) => resolved.push(d.slug.clone()),
                        None if ctx.doc_types.is_empty() => resolved.push(t.clone()),
                        None => issues.push(PlanIssue::warn(
                            at("doc_types"),
                            "doc_type_unknown",
                            format!("unknown type '{t}' ignored"),
                        )),
                    }
                }
                *doc_types = resolved;
            }
            Op::SetDocType { doc_type } => match ctx.doc_type(doc_type) {
                Some(d) => *doc_type = d.slug.clone(),
                None if ctx.doc_types.is_empty() => {}
                None => issues.push(PlanIssue::error(
                    at("doc_type"),
                    "doc_type_unknown",
                    format!("unknown document type '{doc_type}'; known: {}", known_types(ctx)),
                )),
            },
            Op::Ocr { pages, .. } => {
                pages.retain(|p| *p >= 1 && *p <= 5_000);
                let set: BTreeSet<u32> = pages.iter().copied().collect();
                *pages = set.into_iter().collect();
            }
            Op::DefineSchema { base, mode, description, name } => {
                if let Some(d) = description.as_mut() {
                    *d = d.trim().chars().take(2_000).collect();
                }
                if let Some(n) = name.as_mut() {
                    *n = n.trim().chars().take(120).collect();
                }
                if let Some(b) = base.as_deref() {
                    if ctx.schema(b).is_none() && ctx.doc_type(b).is_none() && !ctx.schemas.is_empty() {
                        issues.push(PlanIssue::error(
                            at("base"),
                            "schema_unknown",
                            format!("unknown schema '{b}'; known: {}", known_schemas(ctx)),
                        ));
                    }
                }
                if mode.is_none() {
                    *mode = Some(if base.is_some() { super::SchemaMode::Modify } else { super::SchemaMode::Create });
                }
                if description.as_deref().map(str::trim).unwrap_or("").is_empty()
                    && name.as_deref().map(str::trim).unwrap_or("").is_empty()
                {
                    issues.push(PlanIssue::error(
                        at("description"),
                        "description_required",
                        "define_schema needs a description or a name",
                    ));
                }
            }
            Op::Extract { schema_ref, fields, .. } => {
                let mut schema = None;
                if let Some(r) = schema_ref.clone() {
                    if r.eq_ignore_ascii_case("auto") || r.trim().is_empty() {
                        *schema_ref = None;
                    } else {
                        match ctx.schema(&r) {
                            Some(s) => {
                                *schema_ref = Some(s.id.clone());
                                schema = Some(s);
                            }
                            None if ctx.schemas.is_empty() => {}
                            None => issues.push(PlanIssue::error(
                                at("schema_ref"),
                                "schema_unknown",
                                format!("unknown schema '{r}'; known: {}", known_schemas(ctx)),
                            )),
                        }
                    }
                }
                if schema.is_none() {
                    if let Some(dt) = scope_doc_type.as_deref() {
                        schema = ctx.schema_for_type(dt);
                    }
                }
                if let Some(s) = schema {
                    let mut keep = Vec::new();
                    for f in fields.iter() {
                        match ctx.field(s, f) {
                            Some(fd) => keep.push(fd.key.clone()),
                            None => issues.push(PlanIssue::warn(
                                at("fields"),
                                "field_unknown",
                                format!("'{f}' is not a field of {}; ignored", s.name),
                            )),
                        }
                    }
                    *fields = keep;
                }
            }
            Op::Review { action, field, value, .. } => {
                if *action == ReviewOp::Correct && (field.as_deref().unwrap_or("").trim().is_empty() || value.is_none())
                {
                    issues.push(PlanIssue::error(
                        at("field"),
                        "correct_needs_field",
                        "review correct needs field and value",
                    ));
                }
            }
            Op::Search { query, top_k, .. } => {
                *query = query.trim().chars().take(500).collect();
                if query.is_empty() {
                    issues.push(PlanIssue::error(at("query"), "query_empty", "search needs a query"));
                }
                if let Some(k) = top_k {
                    *k = (*k).clamp(1, 200);
                }
            }
            Op::Answer { question, .. } => {
                *question = question.trim().chars().take(4_000).collect();
                if question.is_empty() {
                    issues.push(PlanIssue::error(at("question"), "question_empty", "answer needs a question"));
                }
            }
            Op::Aggregate { func, field, .. } => {
                let needs_field = *func != AggregateFn::Count;
                let text = field.as_deref().map(str::trim).unwrap_or("");
                if text.is_empty() {
                    if needs_field {
                        // Resolved against the type's schema at execution ("합계" default); warn only.
                        issues.push(PlanIssue::warn(
                            at("field"),
                            "field_default",
                            "no field given; the type's total field is used",
                        ));
                    }
                } else if let Some(dt) = scope_doc_type.as_deref() {
                    if let Some(s) = ctx.schema_for_type(dt) {
                        match ctx.field(s, text) {
                            Some(fd) => *field = Some(fd.key.clone()),
                            None => issues.push(PlanIssue::error(
                                at("field"),
                                "field_unknown",
                                format!(
                                    "'{text}' is not a field of {}; fields: {}",
                                    s.name,
                                    s.fields
                                        .iter()
                                        .map(|f| format!("{}({})", f.key, f.label_ko))
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ),
                            )),
                        }
                    }
                }
            }
            Op::Organize { template: tpl, folder_template, .. } => {
                check_template(tpl.as_deref(), &at("template"), &mut issues);
                if let Some(ft) = folder_template.as_mut() {
                    check_template(Some(ft), &at("folder_template"), &mut issues);
                    check_folder_template(ft, &at("folder_template"), &mut issues);
                }
            }
            Op::Rename { template: tpl, .. } => check_template(Some(tpl), &at("template"), &mut issues),
            Op::Move { folder, .. } => {
                check_template(Some(folder), &at("folder"), &mut issues);
                check_folder_template(folder, &at("folder"), &mut issues);
            }
            Op::Tag { add, remove } => {
                let clean = |v: &mut Vec<String>| {
                    v.iter_mut().for_each(|t| *t = t.trim().chars().take(64).collect());
                    v.retain(|t| !t.is_empty());
                    v.dedup();
                };
                clean(add);
                clean(remove);
                if add.is_empty() && remove.is_empty() {
                    issues.push(PlanIssue::error(at("add"), "tags_empty", "tag needs tags to add or remove"));
                }
            }
            Op::Export { format, path, columns, .. } => {
                if let Some(p) = path.clone() {
                    match workspace_relative(&p) {
                        // Planner exports stay under exports/: a document path would be overwritten otherwise.
                        Some(mut rel) if rel.starts_with("exports/") => {
                            let ext = format!(".{}", format.extension());
                            if !rel.to_lowercase().ends_with(&ext) {
                                rel.push_str(&ext);
                            }
                            *path = Some(rel);
                        }
                        _ => issues.push(PlanIssue::error(
                            at("path"),
                            "path_outside",
                            format!("'{p}' must be a path under exports/"),
                        )),
                    }
                }
                if let Some(dt) = scope_doc_type.as_deref() {
                    if let Some(s) = ctx.schema_for_type(dt) {
                        let mut keep = Vec::new();
                        for c in columns.iter() {
                            match ctx.field(s, c) {
                                Some(fd) => keep.push(fd.key.clone()),
                                None => issues.push(PlanIssue::warn(
                                    at("columns"),
                                    "field_unknown",
                                    format!("'{c}' is not a field of {}; ignored", s.name),
                                )),
                            }
                        }
                        *columns = keep;
                    }
                }
                let _ = ExportFormat::as_str(*format);
            }
            Op::SetModel { id } => {
                if !ctx.model_ids.is_empty() {
                    let key = compact(id);
                    match ctx.model_ids.iter().find(|m| compact(m) == key || compact(m).contains(&key)) {
                        Some(m) => *id = m.clone(),
                        None => issues.push(PlanIssue::error(
                            at("id"),
                            "model_unknown",
                            format!("unknown model '{id}'; available: {}", ctx.model_ids.join(", ")),
                        )),
                    }
                }
            }
            Op::SetSetting { key, value } => check_setting(*key, value, &at("value"), &mut issues),
            Op::Open { target, reference } => {
                *reference = reference.trim().chars().take(300).collect();
                match target {
                    OpenTarget::Screen => {
                        if Screen::parse(reference).is_none() {
                            issues.push(PlanIssue::error(
                                at("ref"),
                                "screen_unknown",
                                format!("unknown screen '{reference}'"),
                            ));
                        }
                    }
                    OpenTarget::Folder => match workspace_relative(reference) {
                        Some(p) => *reference = p,
                        None => issues.push(PlanIssue::error(
                            at("ref"),
                            "path_invalid",
                            format!("'{reference}' is not a workspace folder"),
                        )),
                    },
                    _ => {
                        if reference.is_empty() {
                            issues.push(PlanIssue::error(at("ref"), "ref_empty", "open needs a reference"));
                        }
                    }
                }
            }
            Op::Job { id, .. } => {
                if let Some(v) = id.as_mut() {
                    *v = v.trim().to_string();
                    if v.is_empty() {
                        *id = None;
                    }
                }
            }
            Op::Undo { target } => {
                if let Some(v) = target.as_mut() {
                    *v = v.trim().to_string();
                    if v.is_empty() || v.eq_ignore_ascii_case("last") {
                        *target = None;
                    }
                }
            }
            Op::Clarify { question } => {
                *question = question.trim().chars().take(300).collect();
                if question.is_empty() {
                    issues.push(PlanIssue::error(at("question"), "question_empty", "clarify needs a question"));
                }
            }
            Op::DraftDocument { form_ref, format_description, content, sources, .. } => {
                *content = content.trim().chars().take(6_000).collect();
                if content.is_empty() {
                    issues.push(PlanIssue::error(at("content"), "content_empty", "draft_document needs content"));
                }
                for (name, v) in [("form_ref", form_ref), ("format_description", format_description)] {
                    if let Some(x) = v.as_mut() {
                        *x = x.trim().chars().take(300).collect();
                        if x.is_empty() {
                            *v = None;
                        } else if name == "form_ref"
                            && !ctx.schemas.is_empty()
                            && ctx.schema(x).is_none()
                            && crate::generate::TemplateKind::parse(x).is_none()
                            && !crate::generate::templates().iter().any(|t| {
                                compact(&t.name_ko) == compact(x)
                                    || compact(&t.name_en) == compact(x)
                                    || t.keywords.iter().any(|k| compact(k) == compact(x))
                            })
                        {
                            issues.push(PlanIssue::warn(
                                at("form_ref"),
                                "form_unknown",
                                format!("'{x}' is not a registered form or built-in template; the layout is inferred"),
                            ));
                        }
                    }
                }
                sources.retain(|s| !s.trim().is_empty());
                sources.truncate(20);
                for (i, src) in sources.iter_mut().enumerate() {
                    *src = src.trim().to_string();
                    if src != "selection" && src != "aggregate" && !crate::documents::is_safe_id(src) {
                        issues.push(PlanIssue::error(
                            at(&format!("sources[{i}]")),
                            "source_invalid",
                            format!("'{src}' is not a document id, 'selection' or 'aggregate'"),
                        ));
                    }
                }
            }
            Op::RegisterForm { doc_ref, name, doc_type_slug } => {
                for v in [doc_ref, name, doc_type_slug] {
                    if let Some(x) = v.as_mut() {
                        *x = x.trim().chars().take(200).collect();
                        if x.is_empty() {
                            *v = None;
                        }
                    }
                }
            }
            Op::Explain { selection, question, .. } => {
                for v in [&mut *selection, &mut *question] {
                    if let Some(x) = v.as_mut() {
                        *x = x.trim().chars().take(4_000).collect();
                        if x.is_empty() {
                            *v = None;
                        }
                    }
                }
                if selection.is_none() && question.is_none() {
                    issues.push(PlanIssue::error(
                        at("selection"),
                        "selection_empty",
                        "explain needs a selection or a question",
                    ));
                }
            }
            Op::Compare { doc_ids, aspects } => {
                doc_ids.retain(|d| !d.trim().is_empty());
                doc_ids.dedup();
                if !(2..=5).contains(&doc_ids.len()) {
                    issues.push(PlanIssue::error(at("doc_ids"), "compare_count", "compare needs 2 to 5 documents"));
                }
                aspects.retain(|a| !a.trim().is_empty());
                aspects.truncate(10);
            }
            Op::CreateProject { name, template, doc_types_required } => {
                *name = name.trim().chars().take(120).collect();
                if name.is_empty() {
                    issues.push(PlanIssue::error(at("name"), "name_empty", "create_project needs a name"));
                } else if ctx.project(Some(name)).is_some() {
                    issues.push(PlanIssue::warn(
                        at("name"),
                        "project_exists",
                        format!("project '{name}' already exists"),
                    ));
                }
                if let Some(t) = template.as_mut() {
                    *t = t.trim().chars().take(120).collect();
                    if t.is_empty() {
                        *template = None;
                    }
                }
                for (i, dt) in doc_types_required.iter_mut().enumerate() {
                    match ctx.doc_type(dt) {
                        Some(d) => *dt = d.slug.clone(),
                        None if ctx.doc_types.is_empty() => {}
                        None => issues.push(PlanIssue::error(
                            at(&format!("doc_types_required[{i}]")),
                            "doc_type_unknown",
                            format!("unknown document type '{dt}'; known: {}", known_types(ctx)),
                        )),
                    }
                }
            }
            Op::AddToProject { project, doc_ids } => {
                check_project(project, ctx, &at("project"), &mut issues);
                doc_ids.retain(|d| !d.trim().is_empty());
                if doc_ids.is_empty() && plan_scope_empty {
                    issues.push(PlanIssue::error(
                        at("doc_ids"),
                        "scope_required",
                        "add_to_project needs documents: a selection, a folder / type filter or doc_ids",
                    ));
                }
            }
            Op::DefineRule { project, template, description } => {
                *description = description.trim().chars().take(1_000).collect();
                if description.is_empty() {
                    issues.push(PlanIssue::error(
                        at("description"),
                        "description_empty",
                        "define_rule needs a description",
                    ));
                }
                if let Some(t) = template.as_mut() {
                    *t = t.trim().chars().take(120).collect();
                    if t.is_empty() {
                        *template = None;
                    }
                }
                if template.is_none() {
                    check_project(project, ctx, &at("project"), &mut issues);
                }
            }
            Op::CheckConsistency { project, rules } => {
                check_project(project, ctx, &at("project"), &mut issues);
                rules.retain(|r| !r.trim().is_empty());
            }
            Op::RequestSupplement { project, findings, .. } => {
                check_project(project, ctx, &at("project"), &mut issues);
                findings.retain(|r| !r.trim().is_empty());
            }
            Op::SetRetention { days, .. } => {
                if *days < -1 {
                    *days = -1;
                }
                if *days > 36_500 {
                    issues.push(PlanIssue::error(at("days"), "days_range", "retention must be at most 36,500 days"));
                }
            }
            Op::SetInboxWatch { .. }
            | Op::Validate {}
            | Op::Reindex { .. }
            | Op::Summarize { .. }
            | Op::Delete { .. }
            | Op::SetProfile { .. }
            | Op::SetTheme { .. }
            | Op::SetLanguage { .. }
            | Op::Engine { .. }
            | Op::Redo {}
            | Op::Help {}
            | Op::Noop {} => {}
        }
    }

    // -- derived flags ---------------------------------------------------------
    if plan.operations.iter().any(|o| matches!(o, Op::Clarify { .. })) {
        plan.intent = OpKind::Clarify;
        if plan.clarification_question.is_none() {
            if let Some(Op::Clarify { question }) = plan.operations.iter().find(|o| matches!(o, Op::Clarify { .. })) {
                plan.clarification_question = Some(question.clone());
            }
        }
    } else if !plan.operations.iter().any(|o| o.kind() == plan.intent) {
        plan.recompute_intent();
    }
    plan.requires_confirmation = plan.requires_confirmation || plan.operations.iter().any(Op::needs_confirmation);
    issues
}

fn known_types(ctx: &PlanContext) -> String {
    ctx.doc_types.iter().map(|d| format!("{}({})", d.slug, d.label_ko)).collect::<Vec<_>>().join(", ")
}

/// Resolves a project reference (id / name) to its id; `None` means the project open in the UI,
/// which must exist when project ops are planned.
fn check_project(project: &mut Option<String>, ctx: &PlanContext, path: &str, issues: &mut Vec<PlanIssue>) {
    if let Some(p) = project.as_mut() {
        *p = p.trim().chars().take(120).collect();
        if p.is_empty() {
            *project = None;
        }
    }
    match project.as_deref() {
        Some(r) => match ctx.project(Some(r)) {
            Some(p) => *project = Some(p.id.clone()),
            None if ctx.projects.is_empty() => {}
            None => issues.push(PlanIssue::error(
                path,
                "project_unknown",
                format!(
                    "unknown project '{r}'; known: {}",
                    ctx.projects.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
                ),
            )),
        },
        None => match ctx.current_project.clone() {
            // The open project is the default target; the executor needs the id.
            Some(id) => *project = Some(id),
            None => issues.push(PlanIssue::error(path, "project_required", "no project is open: name the project")),
        },
    }
}

fn known_schemas(ctx: &PlanContext) -> String {
    ctx.schemas.iter().map(|s| format!("{}({})", s.name, s.doc_type)).collect::<Vec<_>>().join(", ")
}

fn check_template(tpl: Option<&str>, path: &str, issues: &mut Vec<PlanIssue>) {
    let Some(t) = tpl else { return };
    if t.trim().is_empty() {
        issues.push(PlanIssue::error(path, "template_empty", "template is empty"));
        return;
    }
    let unknown = template::unknown_variables(t);
    if !unknown.is_empty() {
        issues.push(PlanIssue::error(
            path,
            "template_variable",
            format!("unknown template variable(s) {}; allowed: {}", unknown.join(", "), template::VARIABLES.join(", ")),
        ));
    }
}

fn check_folder_template(t: &str, path: &str, issues: &mut Vec<PlanIssue>) {
    // Variables are substituted later; the literal skeleton must still be a workspace-relative path.
    let skeleton: String = template::parse(t)
        .into_iter()
        .map(|tok| match tok {
            template::Token::Lit(s) => s,
            template::Token::Var(_) => "x".to_string(),
        })
        .collect();
    if workspace_relative(&skeleton).is_none() {
        issues.push(PlanIssue::error(
            path,
            "path_outside",
            format!("'{t}' must be a workspace-relative folder (documents/…)"),
        ));
    }
}

fn check_setting(key: SettingKey, value: &mut Value, path: &str, issues: &mut Vec<PlanIssue>) {
    let bad = |issues: &mut Vec<PlanIssue>, msg: &str| issues.push(PlanIssue::error(path, "setting_value", msg));
    match key {
        SettingKey::RenderDpi => match value.as_f64().or_else(|| value.as_str().and_then(|s| s.trim().parse().ok())) {
            Some(n) if [150.0, 200.0, 300.0].contains(&n) => *value = Value::from(n as u64),
            _ => bad(issues, "render_dpi must be 150, 200 or 300"),
        },
        SettingKey::PiiMasking | SettingKey::InboxWatch => match value {
            Value::Bool(_) => {}
            Value::String(s) if ["true", "on", "켜기", "yes"].contains(&s.trim().to_lowercase().as_str()) => {
                *value = Value::Bool(true)
            }
            Value::String(s) if ["false", "off", "끄기", "no"].contains(&s.trim().to_lowercase().as_str()) => {
                *value = Value::Bool(false)
            }
            _ => bad(issues, "value must be true or false"),
        },
        SettingKey::AutoPipeline => match value.as_str().map(str::trim) {
            Some("none" | "classify_only" | "classify_and_ocr" | "full") => {}
            Some("classify") => *value = Value::from("classify_only"),
            Some("ocr") => *value = Value::from("classify_and_ocr"),
            _ => bad(issues, "auto_pipeline must be none, classify_only, classify_and_ocr or full"),
        },
        SettingKey::ReviewThreshold => {
            match value.as_f64().or_else(|| value.as_str().and_then(|s| s.trim().trim_end_matches('%').parse().ok())) {
                Some(n) if (0.0..=1.0).contains(&n) => *value = Value::from(n),
                Some(n) if (1.0..=100.0).contains(&n) => *value = Value::from(n / 100.0),
                _ => bad(issues, "review_threshold must be between 0 and 1"),
            }
        }
        SettingKey::CacheLimitGb => match value.as_f64().or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
        {
            Some(n) if (0.5..=1000.0).contains(&n) => *value = Value::from(n),
            _ => bad(issues, "cache_limit_gb must be between 0.5 and 1000"),
        },
        SettingKey::JournalDays => {
            match value.as_f64().or_else(|| value.as_str().and_then(|s| s.trim().parse().ok())) {
                Some(n) if (0.0..=36_500.0).contains(&n) => *value = Value::from(n as u64),
                _ => bad(issues, "journal_days must be between 0 and 36500"),
            }
        }
    }
}
