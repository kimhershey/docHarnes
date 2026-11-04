//! Aggregation over extracted fields (design §3.5: `aggregate(fn=sum, field=합계,
//! doc_type=receipt, date_from/to)` → 집계 결과 카드 with the review-pending part
//! kept apart). One schema field is resolved per document type, its latest
//! extracted value is read from `fields`, and sums / counts / averages /
//! extremes are computed over the documents in a plan scope. Decision 16:
//! unreviewed values are excluded from the headline number by default and
//! reported separately ("합계 1,234,500원 = 검토 완료 33건 1,180,000 + 대기 4건
//! 54,500"); `include_unreviewed` folds them in.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::documents::{DocumentRecord, ReviewStatus};
use crate::error::Result;
use crate::organize::template::{format_total, UNKNOWN};
use crate::pipeline::repo::{latest_fields, schema_for_doc_type};
use crate::pipeline::validate::{format_number, normalize_amount};
use crate::pipeline::{FieldDef, FieldKind, FieldRecord, SchemaDef, SchemaRecord};
use crate::planner::scope::{basis_date, resolve};
use crate::planner::{compact, AggregateFn, GroupBy, Scope};

/// Documents listed in a result (the card shows these; the library has the rest).
pub const MAX_DOCUMENTS: usize = 500;

/// One aggregation request (planner `aggregate` op + scope, library 집계 바).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AggregateRequest {
    pub func: AggregateFn,
    /// Field key or label (either language); `None` = the type's total amount
    /// (or the document count for `count`).
    pub field: Option<String>,
    pub group_by: GroupBy,
    /// Fold review-pending values into the headline number.
    pub include_unreviewed: bool,
    pub scope: Scope,
}

impl Default for AggregateRequest {
    fn default() -> Self {
        Self {
            func: AggregateFn::Count,
            field: None,
            group_by: GroupBy::None,
            include_unreviewed: false,
            scope: Scope::default(),
        }
    }
}

/// Sum and count of one review class.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Totals {
    pub value: f64,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggregateGroup {
    /// `YYYY-MM`, vendor, or document type slug (`미상` when unknown).
    pub key: String,
    /// The group's number under the request's function and review setting.
    pub value: f64,
    pub count: u64,
    pub reviewed: Totals,
    pub unreviewed: Totals,
}

/// One document of the scope with the value that went into the result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggregateDocument {
    pub id: String,
    pub name: String,
    pub rel_path: String,
    pub doc_type: Option<String>,
    /// Basis date (`YYYY-MM-DD`).
    pub date: Option<String>,
    /// `doc_date` | `imported_at`
    pub basis: String,
    pub vendor: Option<String>,
    /// `None` = no usable value (counted in `missing`).
    pub value: Option<f64>,
    /// Key the field resolved to for this document's type.
    pub field_key: Option<String>,
    /// `false` when the document or the field is waiting for review.
    pub reviewed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggregateResult {
    pub func: AggregateFn,
    /// Resolved field key (of the first document that resolved one).
    pub field: Option<String>,
    /// Korean label of that field.
    pub field_label: Option<String>,
    pub doc_type: Option<String>,
    /// `doc_date` when every document has a document date, `imported_at` when
    /// none has, `mixed` otherwise.
    pub basis: String,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub include_unreviewed: bool,
    /// The headline number: reviewed values only, or all values when
    /// `include_unreviewed`.
    pub value: f64,
    /// Documents behind `value`.
    pub count: u64,
    pub reviewed: Totals,
    pub unreviewed: Totals,
    /// Documents in scope without a usable value.
    pub missing: u64,
    pub groups: Vec<AggregateGroup>,
    /// Every document in scope (≤ [`MAX_DOCUMENTS`]), by date then name.
    pub documents: Vec<AggregateDocument>,
}

/// Resolves the field a request refers to within one schema: by key or label
/// (either language, case- and whitespace-insensitive), else the type's total
/// amount — the `Amount` field keyed `grand_total` / `total` / `total_amount`,
/// else the last amount whose key contains `total` or whose Korean label
/// contains `합계` / `총액` (the grand total follows the sub-totals on Korean
/// forms), else the first `Amount` field. Only scalar fields qualify (no
/// tables / groups). A text that matches nothing falls back to the total only
/// when it asks for one (`합계`, `총액`, `total` …); otherwise the type has no
/// such field and its documents count as missing.
pub fn resolve_field<'a>(def: &'a SchemaDef, text: Option<&str>) -> Option<&'a FieldDef> {
    if let Some(key) = text.map(compact).filter(|k| !k.is_empty()) {
        let hit = def
            .fields
            .iter()
            .filter(|f| f.kind.is_scalar())
            .find(|f| compact(&f.key) == key || compact(&f.label_ko) == key || compact(&f.label_en) == key);
        if hit.is_some() {
            return hit;
        }
        if !asks_for_total(&key) {
            return None;
        }
    }
    default_total(def)
}

fn asks_for_total(key: &str) -> bool {
    ["합계", "총액", "총합", "총금액", "total", "sum", "amount", "금액"].iter().any(|w| key.contains(w))
}

fn default_total(def: &SchemaDef) -> Option<&FieldDef> {
    let amounts: Vec<&FieldDef> = def.fields.iter().filter(|f| f.kind == FieldKind::Amount).collect();
    if let Some(f) = amounts.iter().find(|f| matches!(f.key.as_str(), "grand_total" | "total" | "total_amount")) {
        return Some(f);
    }
    amounts
        .iter()
        .rev()
        .find(|f| f.key.contains("total") || f.label_ko.contains("합계") || f.label_ko.contains("총액"))
        .or_else(|| amounts.first())
        .copied()
}

/// Numeric value of a document's field row (`fields` of the latest run):
/// `normalized` as a number, else the printed value through the amount
/// normaliser (amount / integer kinds only). Rejected fields have no value.
fn field_value(fields: &[FieldRecord], def: &FieldDef) -> Option<(f64, bool)> {
    let row = fields
        .iter()
        .find(|f| f.row.is_none() && f.path == def.key)
        .or_else(|| fields.iter().find(|f| f.row.is_none() && f.key == def.key))?;
    let printed = row.effective_value()?;
    let strict = |s: &str| s.trim().replace(',', "").parse::<f64>().ok();
    let value = match row.normalized.as_deref().and_then(strict) {
        Some(v) => v,
        None if matches!(def.kind, FieldKind::Amount | FieldKind::Integer) => normalize_amount(printed)?,
        None => strict(printed)?,
    };
    value.is_finite().then_some((value, row.needs_review()))
}

#[derive(Default)]
struct Acc {
    included: Vec<f64>,
    reviewed: Totals,
    unreviewed: Totals,
}

impl Acc {
    fn add(&mut self, value: f64, unreviewed: bool, include_unreviewed: bool) {
        let t = if unreviewed { &mut self.unreviewed } else { &mut self.reviewed };
        t.value += value;
        t.count += 1;
        if !unreviewed || include_unreviewed {
            self.included.push(value);
        }
    }

    fn headline(&self, func: AggregateFn) -> (f64, u64) {
        let n = self.included.len();
        let sum: f64 = self.included.iter().sum();
        let value = match func {
            AggregateFn::Count => n as f64,
            AggregateFn::Sum => sum,
            AggregateFn::Avg if n > 0 => sum / n as f64,
            AggregateFn::Avg => 0.0,
            AggregateFn::Min => self.included.iter().copied().fold(f64::INFINITY, f64::min),
            AggregateFn::Max => self.included.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        };
        (if value.is_finite() { value } else { 0.0 }, n as u64)
    }
}

struct Entry {
    doc: AggregateDocument,
    group: Option<String>,
}

/// Runs an aggregation over the documents of the request's scope.
pub fn aggregate(db: &Db, req: &AggregateRequest) -> Result<AggregateResult> {
    let count_only = req.func == AggregateFn::Count && req.field.is_none();
    // A plain document count covers every document: the card's "영수증 37건" is
    // 검토 완료 33 + 대기 4 (the two classes stay reported separately).
    let include_unreviewed = req.include_unreviewed || count_only;
    let (docs, schemas, fields) = db.with(|c| {
        let docs = resolve(c, &req.scope)?;
        let mut schemas: BTreeMap<String, Option<SchemaRecord>> = BTreeMap::new();
        let mut fields: BTreeMap<String, Vec<FieldRecord>> = BTreeMap::new();
        if !count_only {
            for d in &docs {
                if let Some(dt) = &d.doc_type {
                    if !schemas.contains_key(dt) {
                        schemas.insert(dt.clone(), schema_for_doc_type(c, dt)?);
                    }
                }
                if d.last_run_id.is_some() {
                    fields.insert(d.id.clone(), latest_fields(c, &d.id)?);
                }
            }
        }
        Ok((docs, schemas, fields))
    })?;

    let mut entries: Vec<Entry> = Vec::with_capacity(docs.len());
    let mut total = Acc::default();
    let mut groups: BTreeMap<String, Acc> = BTreeMap::new();
    let mut missing = 0u64;
    let mut resolved: Option<(String, String)> = None;
    let (mut saw_doc_date, mut saw_imported) = (false, false);
    for doc in &docs {
        let (date, basis) = basis_date(doc);
        saw_doc_date |= basis == "doc_date";
        saw_imported |= basis == "imported_at";
        let doc_pending = doc.review_status == ReviewStatus::Pending;
        let (value, field_key, unreviewed) = if count_only {
            (Some(1.0), None, doc_pending)
        } else {
            let def = doc
                .doc_type
                .as_deref()
                .and_then(|dt| schemas.get(dt).and_then(Option::as_ref))
                .and_then(|s| resolve_field(&s.definition, req.field.as_deref()));
            match def {
                Some(def) => {
                    let found = fields.get(&doc.id).and_then(|f| field_value(f, def));
                    if resolved.is_none() && found.is_some() {
                        resolved = Some((def.key.clone(), def.label_ko.clone()));
                    }
                    let needs_review = found.is_some_and(|(_, r)| r);
                    (found.map(|(v, _)| v), Some(def.key.clone()), doc_pending || needs_review)
                }
                None => (None, None, doc_pending),
            }
        };
        let group = group_key(req.group_by, doc, date.as_deref());
        match value {
            Some(v) => {
                total.add(v, unreviewed, include_unreviewed);
                if let Some(g) = &group {
                    groups.entry(g.clone()).or_default().add(v, unreviewed, include_unreviewed);
                }
            }
            None => missing += 1,
        }
        entries.push(Entry {
            doc: AggregateDocument {
                id: doc.id.clone(),
                name: doc.title.clone().unwrap_or_else(|| doc.original_name.clone()),
                rel_path: doc.rel_path.clone(),
                doc_type: doc.doc_type.clone(),
                date,
                basis: basis.to_string(),
                vendor: doc.vendor.clone(),
                value: if count_only { None } else { value },
                field_key,
                reviewed: !unreviewed,
            },
            group,
        });
    }
    // Groups that only hold missing documents still show up (with zero values).
    if req.group_by != GroupBy::None {
        for e in &entries {
            if let Some(g) = &e.group {
                groups.entry(g.clone()).or_default();
            }
        }
    }
    let (value, count) = total.headline(req.func);
    let groups = groups
        .into_iter()
        .map(|(key, acc)| {
            let (value, count) = acc.headline(req.func);
            AggregateGroup { key, value, count, reviewed: acc.reviewed, unreviewed: acc.unreviewed }
        })
        .collect();
    let mut documents: Vec<AggregateDocument> = entries.into_iter().map(|e| e.doc).collect();
    documents.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.name.cmp(&b.name)).then_with(|| a.id.cmp(&b.id)));
    documents.truncate(MAX_DOCUMENTS);
    let basis = match (saw_doc_date, saw_imported) {
        (true, true) => "mixed",
        (false, true) => "imported_at",
        _ => "doc_date",
    };
    let (field, field_label) = resolved.unzip();
    Ok(AggregateResult {
        func: req.func,
        field,
        field_label,
        doc_type: req.scope.filter.doc_type.clone(),
        basis: basis.to_string(),
        date_from: req.scope.filter.date_from.clone(),
        date_to: req.scope.filter.date_to.clone(),
        include_unreviewed: req.include_unreviewed,
        value,
        count,
        reviewed: total.reviewed,
        unreviewed: total.unreviewed,
        missing,
        groups,
        documents,
    })
}

fn group_key(group_by: GroupBy, doc: &DocumentRecord, date: Option<&str>) -> Option<String> {
    let unknown = || UNKNOWN.to_string();
    match group_by {
        GroupBy::None => None,
        GroupBy::Month => Some(date.and_then(|d| d.get(..7)).map(str::to_string).unwrap_or_else(unknown)),
        GroupBy::Vendor => {
            Some(doc.vendor.as_deref().map(str::trim).filter(|v| !v.is_empty()).map_or_else(unknown, str::to_string))
        }
        GroupBy::DocType => {
            Some(doc.doc_type.as_deref().map(str::trim).filter(|v| !v.is_empty()).map_or_else(unknown, str::to_string))
        }
    }
}

/// Number formatting for the result card: integers with thousands separators
/// for sums / counts / extremes, two decimals for averages unless integral.
pub fn format_value(func: AggregateFn, value: f64) -> String {
    if !value.is_finite() {
        return format_number(0.0);
    }
    let sign = if value < 0.0 { "-" } else { "" };
    let abs = value.abs();
    let integral = (abs - abs.round()).abs() < 1e-9;
    if integral {
        return format!("{sign}{}", format_total(abs.round()));
    }
    let text = match func {
        AggregateFn::Avg => format!("{abs:.2}"),
        _ => format_number(abs),
    };
    match text.split_once('.') {
        Some((int, frac)) => format!("{sign}{}.{frac}", format_total(int.parse::<f64>().unwrap_or(0.0))),
        None => format!("{sign}{text}"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::documents::repo as docs;
    use crate::documents::{DocumentRole, DocumentStatus, NewDocument, NewPage, SourceKind};
    use crate::pipeline::repo::{self, NewField, NewRun, RunOutcome};
    use crate::pipeline::ReviewReason;
    use crate::planner::ScopeFilter;

    /// A field of a scripted extraction: `(path, printed value, normalized, kind, review reason)`.
    pub(crate) type Scripted<'a> = (&'a str, &'a str, Option<&'a str>, FieldKind, Option<ReviewReason>);

    pub(crate) struct Seed<'a> {
        pub id: &'a str,
        pub name: &'a str,
        pub doc_type: &'a str,
        pub vendor: &'a str,
        pub doc_date: &'a str,
        pub total: f64,
        pub pending: bool,
        pub fields: &'a [Scripted<'a>],
        /// Page texts (`None` = page without text).
        pub pages: &'a [Option<&'a str>],
    }

    /// Inserts a document with pages and one finished extraction run, the way
    /// `pipeline::run` records them.
    pub(crate) fn seed(db: &Db, s: &Seed<'_>) {
        db.with(|c| {
            docs::insert_document(
                c,
                &NewDocument {
                    id: s.id.into(),
                    rel_path: format!("documents/{}", s.name),
                    original_name: s.name.into(),
                    source_kind: SourceKind::Pdf,
                    content_hash: format!("hash-{}", s.id),
                    size_bytes: 10,
                    file_modified_at: None,
                    role: DocumentRole::Work,
                    import_batch_id: None,
                },
            )?;
            let pages: Vec<NewPage> = s
                .pages
                .iter()
                .enumerate()
                .map(|(i, text)| NewPage {
                    page_no: i as u32 + 1,
                    width_px: 200,
                    height_px: 300,
                    dpi: Some(200),
                    text: text.map(str::to_string),
                    is_blank: Some(false),
                    ..Default::default()
                })
                .collect();
            docs::replace_pages(c, s.id, &pages, Some(200), false, None)?;
            docs::set_status(c, s.id, DocumentStatus::Rendered, None)?;
            docs::set_doc_type(c, s.id, Some(s.doc_type), Some("manual"))?;
            let schema = repo::schema_for_doc_type(c, s.doc_type)?.expect("builtin schema");
            let run_id = format!("run-{}", s.id);
            repo::insert_run(
                c,
                &NewRun {
                    id: &run_id,
                    document_id: s.id,
                    schema: Some(&schema),
                    doc_type: Some(s.doc_type),
                    model: "scripted",
                    profile: "balanced",
                    precise: false,
                },
            )?;
            let fields: Vec<NewField> = s
                .fields
                .iter()
                .map(|(path, value, normalized, kind, reason)| {
                    let (key, row) = match path.split_once('[') {
                        Some((_, rest)) => {
                            let (idx, col) = rest.split_once("].").unwrap();
                            (col.to_string(), Some(idx.parse::<u32>().unwrap()))
                        }
                        None => (path.rsplit('.').next().unwrap().to_string(), None),
                    };
                    let def = match row {
                        Some(_) => schema
                            .definition
                            .table(path.split('[').next().unwrap())
                            .and_then(|t| t.fields.iter().find(|c| c.key == key)),
                        None => schema.definition.field(path),
                    };
                    let label = def
                        .map(|f| (f.label_ko.clone(), f.label_en.clone()))
                        .unwrap_or_else(|| (key.clone(), key.clone()));
                    NewField {
                        path: path.to_string(),
                        key,
                        row,
                        label_ko: label.0,
                        label_en: label.1,
                        kind: *kind,
                        required: false,
                        value: Some(value.to_string()),
                        normalized: normalized.map(str::to_string),
                        page: Some(1),
                        bbox: None,
                        evidence: Some(format!("{value} 근거")),
                        confidence: if reason.is_some() { 0.5 } else { 0.95 },
                        model_confidence: Some(90),
                        review_reason: *reason,
                        validation: Vec::new(),
                    }
                })
                .collect();
            repo::insert_fields(c, &run_id, s.id, &fields)?;
            let review_count = fields.iter().filter(|f| f.review_reason.is_some()).count() as u32;
            let min_confidence = fields.iter().map(|f| f.confidence).fold(1.0, f64::min);
            repo::finish_run(
                c,
                &run_id,
                &RunOutcome {
                    status: "done",
                    error: None,
                    duration_ms: 10,
                    prompt_tokens: 1,
                    completion_tokens: 1,
                    pages_done: 1,
                    result_json: None,
                    confidence: Some(min_confidence),
                    field_count: fields.len() as u32,
                    review_count,
                },
            )?;
            repo::set_extracted(
                c,
                s.id,
                &run_id,
                Some(min_confidence),
                s.pending,
                Some(s.vendor),
                Some(s.doc_date),
                Some(s.total),
            )
        })
        .unwrap();
    }

    pub(crate) const RECEIPT_TEXT: &str = "# 스타벅스 역삼점\n2026-10-14 12:31\n<table><tr><th>품명</th><th>수량</th><th>금액</th></tr><tr><td>아메리카노</td><td>2</td><td>9,000</td></tr><tr><td>쿠키</td><td>1</td><td>3,500</td></tr></table>\n공급가액 11,364\n부가세 1,136\n합계 12,500";

    /// Two receipts (one reviewed, one pending) and one tax invoice.
    pub(crate) fn seed_all(db: &Db) {
        use FieldKind::*;
        seed(
            db,
            &Seed {
                id: "r1",
                name: "a_receipt1.pdf",
                doc_type: "receipt",
                vendor: "스타벅스 역삼점",
                doc_date: "2026-10-14",
                total: 12_500.0,
                pending: false,
                fields: &[
                    ("store_name", "스타벅스 역삼점", Some("스타벅스 역삼점"), String, None),
                    ("datetime", "2026-10-14 12:31", Some("2026-10-14"), Date, None),
                    ("supply_amount", "11,364", Some("11364"), Amount, None),
                    ("vat", "1,136", Some("1136"), Amount, None),
                    ("total", "12,500", Some("12500"), Amount, None),
                    ("items[0].name", "아메리카노", Some("아메리카노"), String, None),
                    ("items[0].quantity", "2", Some("2"), Integer, None),
                    ("items[0].amount", "9,000", Some("9000"), Amount, None),
                    ("items[1].name", "쿠키", Some("쿠키"), String, None),
                    ("items[1].amount", "3,500", Some("3500"), Amount, None),
                ],
                pages: &[Some(RECEIPT_TEXT)],
            },
        );
        seed(
            db,
            &Seed {
                id: "r2",
                name: "b_receipt2.pdf",
                doc_type: "receipt",
                vendor: "이디야",
                doc_date: "2026-10-02",
                total: 8_900.0,
                pending: true,
                fields: &[
                    ("store_name", "이디야", Some("이디야"), String, None),
                    ("datetime", "2026-10-02", Some("2026-10-02"), Date, None),
                    ("vat", "809", Some("809"), Amount, None),
                    ("total", "8,900", None, Amount, Some(ReviewReason::LowConfidence)),
                ],
                pages: &[None],
            },
        );
        seed(
            db,
            &Seed {
                id: "t1",
                name: "c_invoice.pdf",
                doc_type: "tax_invoice",
                vendor: "한빛상사",
                doc_date: "2026-09-30",
                total: 1_100_000.0,
                pending: false,
                fields: &[
                    ("issue_date", "2026.09.30", Some("2026-09-30"), Date, None),
                    ("supplier.name", "한빛상사", Some("한빛상사"), String, None),
                    ("supply_total", "1,000,000", Some("1000000"), Amount, None),
                    ("tax_total", "100,000", Some("100000"), Amount, None),
                    ("grand_total", "1,100,000", Some("1100000"), Amount, None),
                ],
                pages: &[Some("세금계산서 공급가액 1,000,000 세액 100,000 합계 1,100,000")],
            },
        );
    }

    fn db() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.with(repo::ensure_builtins).unwrap();
        seed_all(&db);
        db
    }

    fn receipts(field: &str, func: AggregateFn, include_unreviewed: bool) -> AggregateRequest {
        AggregateRequest {
            func,
            field: Some(field.into()),
            include_unreviewed,
            scope: Scope {
                filter: ScopeFilter { doc_type: Some("receipt".into()), ..Default::default() },
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn sum_separates_reviewed_from_pending() {
        let db = db();
        let r = aggregate(&db, &receipts("합계", AggregateFn::Sum, false)).unwrap();
        assert_eq!(r.field.as_deref(), Some("total"));
        assert_eq!(r.field_label.as_deref(), Some("합계"));
        assert_eq!(r.doc_type.as_deref(), Some("receipt"));
        assert_eq!(r.basis, "doc_date");
        assert_eq!((r.value, r.count, r.missing), (12_500.0, 1, 0));
        assert_eq!(r.reviewed, Totals { value: 12_500.0, count: 1 });
        assert_eq!(r.unreviewed, Totals { value: 8_900.0, count: 1 });
        assert_eq!(r.documents.len(), 2);
        assert_eq!(r.documents[0].id, "r2", "sorted by date");
        assert!(!r.documents[0].reviewed && r.documents[1].reviewed);
        assert_eq!(r.documents[0].value, Some(8_900.0));
        assert!(r.groups.is_empty());

        let all = aggregate(&db, &receipts("합계", AggregateFn::Sum, true)).unwrap();
        assert_eq!((all.value, all.count), (21_400.0, 2));
        assert!(all.include_unreviewed);
        assert_eq!(format_value(all.func, all.value), "21,400");
    }

    #[test]
    fn count_avg_min_max() {
        let db = db();
        let count = aggregate(&db, &AggregateRequest::default()).unwrap();
        assert_eq!((count.value, count.count, count.missing), (3.0, 3, 0));
        assert_eq!(count.reviewed.count, 2);
        assert_eq!(count.unreviewed.count, 1);
        assert!(count.field.is_none() && count.documents.iter().all(|d| d.value.is_none()));

        let counted = aggregate(&db, &receipts("total", AggregateFn::Count, false)).unwrap();
        assert_eq!((counted.value, counted.count), (1.0, 1), "only the reviewed receipt has a counted value");
        let avg = aggregate(&db, &receipts("합계", AggregateFn::Avg, true)).unwrap();
        assert_eq!(avg.value, 10_700.0);
        let min = aggregate(&db, &receipts("합계", AggregateFn::Min, true)).unwrap();
        assert_eq!(min.value, 8_900.0);
        let max = aggregate(&db, &receipts("합계", AggregateFn::Max, true)).unwrap();
        assert_eq!(max.value, 12_500.0);
        let max_reviewed = aggregate(&db, &receipts("합계", AggregateFn::Max, false)).unwrap();
        assert_eq!(max_reviewed.value, 12_500.0);
    }

    #[test]
    fn groups_by_month_vendor_and_type() {
        let db = db();
        let req = AggregateRequest {
            func: AggregateFn::Sum,
            field: Some("합계".into()),
            group_by: GroupBy::Month,
            include_unreviewed: true,
            ..Default::default()
        };
        let r = aggregate(&db, &req).unwrap();
        let keys: Vec<(&str, f64, u64)> = r.groups.iter().map(|g| (g.key.as_str(), g.value, g.count)).collect();
        assert_eq!(keys, vec![("2026-09", 1_100_000.0, 1), ("2026-10", 21_400.0, 2)]);
        assert_eq!(r.groups[1].unreviewed, Totals { value: 8_900.0, count: 1 });

        let by_vendor = aggregate(&db, &AggregateRequest { group_by: GroupBy::Vendor, ..req.clone() }).unwrap();
        let keys: Vec<&str> = by_vendor.groups.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(keys, vec!["스타벅스 역삼점", "이디야", "한빛상사"]);

        let by_type = aggregate(&db, &AggregateRequest { group_by: GroupBy::DocType, ..req }).unwrap();
        let keys: Vec<(&str, f64)> = by_type.groups.iter().map(|g| (g.key.as_str(), g.value)).collect();
        assert_eq!(keys, vec![("receipt", 21_400.0), ("tax_invoice", 1_100_000.0)]);
        // Group keys for documents without a vendor fall back to the placeholder.
        db.with(|c| c.execute("UPDATE documents SET vendor = NULL WHERE id = 't1'", [])).unwrap();
        let by_vendor = aggregate(&db, &AggregateRequest { group_by: GroupBy::Vendor, ..Default::default() }).unwrap();
        assert!(by_vendor.groups.iter().any(|g| g.key == UNKNOWN && g.count == 1));
    }

    #[test]
    fn total_resolves_per_type_and_unknown_fields_are_missing() {
        let db = db();
        // "합계" is the receipt's `total` label; the tax invoice has no such label, so its
        // grand total (`grand_total`) stands in — both types are included.
        let req = AggregateRequest { func: AggregateFn::Sum, field: Some("합계".into()), ..Default::default() };
        let r = aggregate(&db, &req).unwrap();
        assert_eq!((r.value, r.count, r.missing), (1_112_500.0, 2, 0));
        let key = |id: &str| r.documents.iter().find(|d| d.id == id).unwrap().field_key.clone();
        assert_eq!(key("r1").as_deref(), Some("total"));
        assert_eq!(key("t1").as_deref(), Some("grand_total"));
        assert_eq!(r.field.as_deref(), Some("total"), "first resolved document names the field");
        // A field the tax invoice does not have at all → that document is missing.
        let vat = aggregate(&db, &AggregateRequest { field: Some("부가세".into()), ..req.clone() }).unwrap();
        assert_eq!((vat.value, vat.count, vat.missing), (1_136.0, 1, 1));
        assert_eq!(vat.unreviewed, Totals { value: 809.0, count: 1 });
        assert!(vat.documents.iter().find(|d| d.id == "t1").unwrap().field_key.is_none());
        // A rejected value is no value.
        db.with(|c| c.execute("UPDATE fields SET status = 'rejected' WHERE document_id = 'r1' AND path = 'total'", []))
            .unwrap();
        let r = aggregate(&db, &req).unwrap();
        assert_eq!((r.value, r.missing), (1_100_000.0, 1));
        // A document without a type (or whose type has no schema) is missing too.
        db.with(|c| c.execute("UPDATE documents SET doc_type = NULL WHERE id = 't1'", [])).unwrap();
        let r = aggregate(&db, &req).unwrap();
        assert_eq!(r.missing, 2);
    }

    #[test]
    fn resolves_fields_by_key_and_labels() {
        let db = Db::open_in_memory().unwrap();
        db.with(repo::ensure_builtins).unwrap();
        let receipt = db.with(|c| repo::schema_for_doc_type(c, "receipt")).unwrap().unwrap().definition;
        let invoice = db.with(|c| repo::schema_for_doc_type(c, "tax_invoice")).unwrap().unwrap().definition;
        let key = |def: &SchemaDef, t: Option<&str>| resolve_field(def, t).map(|f| f.key.clone());
        assert_eq!(key(&receipt, Some("합계")).as_deref(), Some("total"));
        assert_eq!(key(&receipt, Some("TOTAL")).as_deref(), Some("total"));
        assert_eq!(key(&receipt, Some(" 공급 가액 ")).as_deref(), Some("supply_amount"));
        assert_eq!(key(&receipt, Some("Supply amount")).as_deref(), Some("supply_amount"));
        assert_eq!(key(&receipt, None).as_deref(), Some("total"));
        assert_eq!(key(&invoice, None).as_deref(), Some("grand_total"));
        assert_eq!(key(&invoice, Some("합계")).as_deref(), Some("grand_total"), "total-ish text → the type's total");
        assert_eq!(key(&invoice, Some("총액")).as_deref(), Some("grand_total"));
        assert_eq!(key(&invoice, Some("세액 합계")).as_deref(), Some("tax_total"), "exact label wins");
        assert_eq!(key(&invoice, Some("부가세")), None);
        assert_eq!(key(&receipt, Some("품목")), None, "tables are not aggregated");
        assert_eq!(key(&receipt, Some("")).as_deref(), Some("total"), "blank text = default");
    }

    #[test]
    fn date_filter_through_scope() {
        let db = db();
        let req = |from: Option<&str>, to: Option<&str>| AggregateRequest {
            func: AggregateFn::Sum,
            field: Some("합계".into()),
            include_unreviewed: true,
            scope: Scope {
                filter: ScopeFilter {
                    date_from: from.map(str::to_string),
                    date_to: to.map(str::to_string),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let october = aggregate(&db, &req(Some("2026-10-01"), Some("2026-10-31"))).unwrap();
        assert_eq!((october.value, october.count), (21_400.0, 2));
        assert_eq!(october.date_from.as_deref(), Some("2026-10-01"));
        let late = aggregate(&db, &req(Some("2026-10-10"), None)).unwrap();
        assert_eq!(late.documents.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), vec!["r1"]);
        let early = aggregate(&db, &req(None, Some("2026-09-30"))).unwrap();
        assert_eq!((early.value, early.count), (1_100_000.0, 1));
        // A document without a document date is filed under its import date ("mixed" basis).
        db.with(|c| c.execute("UPDATE documents SET doc_date = NULL WHERE id = 'r2'", [])).unwrap();
        let all = aggregate(&db, &req(None, None)).unwrap();
        assert_eq!(all.basis, "mixed");
        assert_eq!(all.documents.iter().find(|d| d.id == "r2").unwrap().basis, "imported_at");
    }

    #[test]
    fn formats_values() {
        assert_eq!(format_value(AggregateFn::Sum, 1_234_500.0), "1,234,500");
        assert_eq!(format_value(AggregateFn::Count, 3.0), "3");
        assert_eq!(format_value(AggregateFn::Avg, 10_700.0), "10,700");
        assert_eq!(format_value(AggregateFn::Avg, 10_716.666), "10,716.67");
        assert_eq!(format_value(AggregateFn::Avg, -0.25), "-0.25");
        assert_eq!(format_value(AggregateFn::Sum, 1_234.5), "1,234.5");
        assert_eq!(format_value(AggregateFn::Min, -1_234_567.0), "-1,234,567");
        assert_eq!(format_value(AggregateFn::Max, f64::NAN), "0");
    }
}
