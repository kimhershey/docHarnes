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
    compact, settings_catalog, AggregateFn, ExportFormat, ImportFrom, ModelAction, Op, OpKind, OpenTarget, Plan,
    PlanContext, PlanIssue, ReferenceAction, ReviewOp, SchemaAction, TemplateAction, MAX_OPS,
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
        // 0.6.0: the document-changing ops name their targets too.
        Op::Rotate { .. }
        | Op::Rerender { .. }
        | Op::Split { .. }
        | Op::Merge { .. }
        | Op::SetTitle { .. }
        | Op::EditText { .. }
        | Op::SetRole { .. }
        | Op::SetVersion { .. } => true,
        Op::ManageReference { action: ReferenceAction::Add | ReferenceAction::Remove, .. } => true,
        Op::RemoveFromProject { doc_ids, .. } => doc_ids.is_empty(),
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
    // Project scope (plan §6): a name from the model becomes the id; an unknown project is an error.
    if let Some(p) = f.project_id.as_deref().map(str::trim) {
        if p.is_empty() {
            f.project_id = None;
        } else {
            match ctx.project(Some(p)) {
                Some(known) => f.project_id = Some(known.id.clone()),
                None if ctx.projects.is_empty() => {}
                None => issues.push(PlanIssue::error(
                    "scope.filter.project_id",
                    "project_unknown",
                    format!(
                        "unknown project '{p}'; known: {}",
                        ctx.projects.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(", ")
                    ),
                )),
            }
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
    // The assistant's "현재 프로젝트" scope (0.5.0) is the default scope of a plan that names no
    // documents / folder / project — also for model plans, which may leave the filter out.
    // `add_to_project` and `import` bring documents in from outside, so they keep their scope.
    if plan.scope.filter.project_id.is_none() && plan.scope.doc_ids.is_empty() && plan.scope.folder.is_none() {
        if let Some(p) = ctx.scope_project.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            let brings_in = plan.operations.iter().any(|op| matches!(op, Op::AddToProject { .. } | Op::Import { .. }));
            if !brings_in && ctx.projects.iter().any(|x| x.id == p) {
                plan.scope.filter.project_id = Some(p.to_string());
            }
        }
    }
    let scope_doc_type = plan.scope.filter.doc_type.clone();
    let plan_scope_empty = plan.scope.is_empty();
    for (i, op) in plan.operations.iter_mut().enumerate() {
        let at = |field: &str| format!("operations[{i}].{field}");
        match op {
            Op::Import { source, path, paths, .. } => {
                for p in paths.iter_mut() {
                    *p = p.trim().chars().take(1_000).collect();
                }
                paths.retain(|p| !p.is_empty());
                paths.truncate(200);
                if *source == ImportFrom::Files && paths.is_empty() {
                    issues.push(PlanIssue::error(
                        at("paths"),
                        "paths_required",
                        "import from files needs the file paths (type them or drop the files); otherwise use source=inbox or folder",
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
            Op::SetSetting { key, value } => check_setting(key, value, &at("key"), &at("value"), &mut issues),
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
                            && !crate::generate::templates_all().iter().any(|t| {
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
            Op::RegisterForm { doc_ref, name, doc_type_slug, doc_ids } => {
                for v in [doc_ref, name, doc_type_slug] {
                    if let Some(x) = v.as_mut() {
                        *x = x.trim().chars().take(200).collect();
                        if x.is_empty() {
                            *v = None;
                        }
                    }
                }
                let mut seen: Vec<String> = Vec::new();
                for d in doc_ids.iter() {
                    let d: String = d.trim().chars().take(200).collect();
                    if !d.is_empty() && !seen.contains(&d) {
                        seen.push(d);
                    }
                }
                *doc_ids = seen;
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
                let mut seen = BTreeSet::new();
                doc_ids.retain(|d| seen.insert(d.clone()));
                for (j, d) in doc_ids.iter().enumerate() {
                    if !crate::documents::is_safe_id(d) {
                        issues.push(PlanIssue::error(
                            at(&format!("doc_ids[{j}]")),
                            "doc_id_invalid",
                            format!("'{d}' is not a document id"),
                        ));
                    }
                }
                if !(2..=5).contains(&doc_ids.len()) {
                    issues.push(PlanIssue::error(at("doc_ids"), "compare_count", "compare needs 2 to 5 documents"));
                }
                aspects.retain(|a| !a.trim().is_empty());
                aspects.truncate(10);
            }
            Op::CreateProject { name, template, doc_types_required, start_date, end_date } => {
                for (label, d) in [("start_date", start_date), ("end_date", end_date)] {
                    if let Some(v) = d.as_mut() {
                        *v = v.trim().to_string();
                        if v.is_empty() {
                            *d = None;
                        } else if crate::projects::parse_date(v).is_none() {
                            issues.push(PlanIssue::error(
                                at(label),
                                "date_format",
                                format!("'{v}' is not a date (YYYY-MM-DD)"),
                            ));
                        }
                    }
                }
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
            Op::AddToProject { project, doc_ids, .. } => {
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
            // -- 0.6.0 -----------------------------------------------------------------------
            Op::Rotate { degrees, pages } => {
                *degrees %= 360;
                if *degrees == 0 || *degrees % 90 != 0 {
                    issues.push(PlanIssue::error(at("degrees"), "degrees_invalid", "rotate needs 90, 180 or 270"));
                }
                pages.retain(|p| *p >= 1);
                pages.sort_unstable();
                pages.dedup();
            }
            Op::Rerender { dpi } => {
                if let Some(d) = dpi {
                    if !(72..=600).contains(d) {
                        issues.push(PlanIssue::error(at("dpi"), "dpi_range", "dpi must be between 72 and 600"));
                    }
                }
            }
            Op::Split { boundaries, every } => {
                boundaries.retain(|b| *b >= 2);
                boundaries.sort_unstable();
                boundaries.dedup();
                if let Some(n) = every {
                    if *n == 0 {
                        *every = None;
                    }
                }
                if boundaries.is_empty() && every.is_none() {
                    issues.push(PlanIssue::error(
                        at("boundaries"),
                        "boundaries_required",
                        "split needs the pages that start a new document (boundaries) or a page count (every)",
                    ));
                }
            }
            Op::Merge { name } => trim_opt(name, 200),
            Op::SetTitle { title } => {
                *title = title.trim().chars().take(300).collect();
                if title.is_empty() {
                    issues.push(PlanIssue::error(at("title"), "title_empty", "set_title needs a title"));
                }
            }
            Op::EditText { find, replace, pages } => {
                *find = find.chars().take(500).collect();
                *replace = replace.chars().take(2_000).collect();
                if find.trim().is_empty() {
                    issues.push(PlanIssue::error(at("find"), "find_empty", "edit_text needs the text to find"));
                }
                pages.retain(|p| *p >= 1);
                pages.sort_unstable();
                pages.dedup();
            }
            Op::SetRole { .. } => {}
            Op::ManageReference { action, group, name, project } => {
                trim_opt(group, 200);
                trim_opt(name, 200);
                if project.is_some() {
                    check_project(project, ctx, &at("project"), &mut issues);
                } else if group.is_none() && matches!(action, ReferenceAction::Add | ReferenceAction::Remove) {
                    // No group named: the open project's group (plan A), else ask.
                    match ctx.scope_project.clone().or_else(|| ctx.current_project.clone()) {
                        Some(p) => *project = Some(p),
                        None => issues.push(PlanIssue::error(
                            at("group"),
                            "group_required",
                            "name the reference group (or open a project)",
                        )),
                    }
                }
                match action {
                    ReferenceAction::Create if name.is_none() && group.is_none() => {
                        issues.push(PlanIssue::error(at("name"), "name_required", "create needs the group's name"))
                    }
                    ReferenceAction::Rename if name.is_none() => {
                        issues.push(PlanIssue::error(at("name"), "name_required", "rename needs the new name"))
                    }
                    ReferenceAction::Rename
                    | ReferenceAction::Delete
                    | ReferenceAction::Enable
                    | ReferenceAction::Disable
                        if group.is_none() && project.is_none() =>
                    {
                        issues.push(PlanIssue::error(at("group"), "group_required", "name the reference group"))
                    }
                    _ => {}
                }
            }
            Op::DefineTerm { term, definition, synonyms, .. } => {
                *term = term.trim().chars().take(120).collect();
                if term.is_empty() {
                    issues.push(PlanIssue::error(at("term"), "term_empty", "define_term needs the term"));
                }
                trim_opt(definition, 2_000);
                for s in synonyms.iter_mut() {
                    *s = s.trim().chars().take(120).collect();
                }
                synonyms.retain(|s| !s.is_empty());
                synonyms.truncate(20);
            }
            Op::SetVersion { group_key, label, effective_date, .. } => {
                trim_opt(group_key, 200);
                trim_opt(label, 60);
                if let Some(d) = effective_date.as_mut() {
                    *d = d.trim().to_string();
                    if d.is_empty() {
                        *effective_date = None;
                    } else if chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_err() {
                        issues.push(PlanIssue::error(
                            at("effective_date"),
                            "date_invalid",
                            "effective_date must be YYYY-MM-DD",
                        ));
                    }
                }
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
                check_project(project, ctx, &at("project"), &mut issues);
                trim_opt(name, 120);
                trim_opt(template, 120);
                trim_opt(owner, 120);
                trim_opt(intake_no, 120);
                trim_opt(notes, 4_000);
                trim_opt(inbox_folder, 300);
                for (label, d) in [("start_date", &mut *start_date), ("end_date", &mut *end_date)] {
                    if let Some(v) = d.as_mut() {
                        *v = v.trim().to_string();
                        if !v.is_empty() && chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d").is_err() {
                            issues.push(PlanIssue::error(
                                at(label),
                                "date_invalid",
                                format!("{label} must be YYYY-MM-DD (empty clears)"),
                            ));
                        }
                    }
                }
                if let Some(k) = keywords.as_mut() {
                    for s in k.iter_mut() {
                        *s = s.trim().chars().take(80).collect();
                    }
                    k.retain(|s| !s.is_empty());
                    k.truncate(50);
                }
                let changes = name.is_some()
                    || template.is_some()
                    || start_date.is_some()
                    || end_date.is_some()
                    || owner.is_some()
                    || intake_no.is_some()
                    || notes.is_some()
                    || keywords.is_some()
                    || inbox_folder.is_some();
                if !changes {
                    issues.push(PlanIssue::error(
                        at("name"),
                        "nothing_to_change",
                        "update_project needs at least one field to change",
                    ));
                }
            }
            Op::DeleteProject { project } => check_project(project, ctx, &at("project"), &mut issues),
            Op::RemoveFromProject { project, doc_ids } => {
                check_project(project, ctx, &at("project"), &mut issues);
                doc_ids.retain(|d| !d.trim().is_empty());
            }
            Op::ManageTemplate { action, template, name, doc_types_required, description } => {
                trim_opt(template, 120);
                trim_opt(name, 120);
                trim_opt(description, 2_000);
                let mut resolved = Vec::new();
                for t in doc_types_required.iter() {
                    match ctx.doc_type(t) {
                        Some(d) => resolved.push(d.slug.clone()),
                        None if ctx.doc_types.is_empty() => resolved.push(t.clone()),
                        None => issues.push(PlanIssue::warn(
                            at("doc_types_required"),
                            "doc_type_unknown",
                            format!("unknown type '{t}' ignored"),
                        )),
                    }
                }
                *doc_types_required = resolved;
                if template.is_none() && name.is_none() {
                    issues.push(PlanIssue::error(
                        at("name"),
                        "name_required",
                        "manage_template needs the template's name",
                    ));
                }
                if *action == TemplateAction::Save && template.is_none() && doc_types_required.is_empty() {
                    issues.push(PlanIssue::warn(
                        at("doc_types_required"),
                        "template_empty",
                        "the new template lists no required document types",
                    ));
                }
            }
            Op::ClassifyProjects { .. } => {}
            Op::ManageSchema { action, schema, name } => {
                *schema = schema.trim().chars().take(120).collect();
                if schema.is_empty() {
                    issues.push(PlanIssue::error(
                        at("schema"),
                        "schema_required",
                        "manage_schema needs the schema (name, id or type)",
                    ));
                }
                trim_opt(name, 120);
                if *action == SchemaAction::Rename && name.is_none() {
                    issues.push(PlanIssue::error(at("name"), "name_required", "rename needs the new name"));
                }
            }
            Op::ManageModel { action, id, tier, backend, path } => {
                trim_opt(id, 120);
                trim_opt(tier, 40);
                trim_opt(backend, 40);
                trim_opt(path, 1_000);
                match action {
                    ModelAction::SetTier => match tier.as_deref() {
                        Some(t) if t == "auto" || settings_catalog::TIERS.contains(&t) => {}
                        _ => issues.push(PlanIssue::error(
                            at("tier"),
                            "tier_invalid",
                            format!("tier must be auto or one of {}", settings_catalog::TIERS.join(", ")),
                        )),
                    },
                    ModelAction::SetBackend => match backend.as_deref() {
                        Some(b) if settings_catalog::BACKENDS.contains(&b) => {}
                        _ => issues.push(PlanIssue::error(
                            at("backend"),
                            "backend_invalid",
                            format!("backend must be one of {}", settings_catalog::BACKENDS.join(", ")),
                        )),
                    },
                    ModelAction::Import => {
                        if path.is_none() {
                            issues.push(PlanIssue::error(
                                at("path"),
                                "path_required",
                                "import needs the model folder or GGUF path",
                            ));
                        }
                    }
                    ModelAction::Download
                    | ModelAction::Delete
                    | ModelAction::Verify
                    | ModelAction::Load
                    | ModelAction::Unload => {
                        if let Some(m) = id.as_deref() {
                            if !ctx.model_ids.is_empty()
                                && !ctx.model_ids.iter().any(|k| k == m)
                                && *action == ModelAction::Download
                            {
                                issues.push(PlanIssue::warn(
                                    at("id"),
                                    "model_unknown",
                                    format!("'{m}' is not in the registry; known: {}", ctx.model_ids.join(", ")),
                                ));
                            }
                        } else {
                            issues.push(PlanIssue::error(at("id"), "model_id_required", "name the model (id)"));
                        }
                    }
                    ModelAction::Benchmark => {}
                }
            }
            Op::Workspace { action, path } => {
                trim_opt(path, 1_000);
                if matches!(
                    action,
                    super::WorkspaceAction::Open | super::WorkspaceAction::Create | super::WorkspaceAction::Forget
                ) && path.is_none()
                {
                    issues.push(PlanIssue::error(
                        at("path"),
                        "path_required",
                        "workspace open / create / forget needs the folder path",
                    ));
                }
            }
            Op::Maintenance { .. } => {}
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

/// `set_setting`: the key must be a catalog path (0.1–0.5 short keys are mapped), the value is
/// coerced to the key's type (booleans from "켜/끄", percentages, Korean enum names).
fn check_setting(key: &mut String, value: &mut Value, key_path: &str, value_path: &str, issues: &mut Vec<PlanIssue>) {
    match settings_catalog::find(key) {
        Some((canonical, spec)) => {
            *key = canonical.to_string();
            if let Err(msg) = settings_catalog::coerce(spec, value) {
                issues.push(PlanIssue::error(value_path, "setting_value", msg));
            }
        }
        None => issues.push(PlanIssue::error(
            key_path,
            "setting_unknown",
            format!(
                "unknown setting '{key}'; keys: {}",
                settings_catalog::SETTINGS.iter().map(|s| s.key).collect::<Vec<_>>().join(", ")
            ),
        )),
    }
}

fn trim_opt(v: &mut Option<String>, max: usize) {
    if let Some(s) = v.as_mut() {
        *s = s.trim().chars().take(max).collect();
        if s.is_empty() {
            *v = None;
        }
    }
}
