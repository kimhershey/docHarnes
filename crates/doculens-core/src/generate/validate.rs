//! Reverse validation of a draft (§3.13 "검증"): the pipeline validators are
//! applied to the generated values — amounts / dates / 사업자등록번호 are
//! normalised and checked, item rows must satisfy 수량 × 단가 = 공급가액, the
//! 공급가액 / 세액 (10 %) / 합계 totals must agree with the item table (empty
//! totals are filled in, conflicts are flagged with the computed value as the
//! suggestion), required fields must be present, body paragraphs that state
//! money or dates without a citation get a non-blocking `uncited_claim`, and
//! field keys unknown to a built-in template are reported.

use std::sync::OnceLock;

use chrono::Utc;
use regex::Regex;

use crate::pipeline::schema::FieldKind;
use crate::pipeline::validate::{
    brn_digits, brn_valid, format_brn, format_number, normalize_amount, normalize_date, normalize_integer,
    normalize_phone,
};

use super::templates::TemplateSpec;
use super::{Draft, DraftBlock, DraftIssue, DraftTable, TemplateRef};

/// Integer with thousands separators (`1,250,000`; negatives keep the sign).
pub fn format_won(n: f64) -> String {
    let i = n.round() as i64;
    let s = i.abs().to_string();
    let mut out = String::new();
    for (k, c) in s.chars().enumerate() {
        if k > 0 && (s.len() - k) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if i < 0 {
        format!("-{out}")
    } else {
        out
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1.0
}

fn issue(
    path: impl Into<String>,
    code: &str,
    message: String,
    blocking: bool,
    suggestion: Option<String>,
) -> DraftIssue {
    DraftIssue { path: path.into(), code: code.into(), message, blocking, suggestion }
}

fn tr(ko: bool, k: &str, e: &str) -> String {
    if ko {
        k.to_string()
    } else {
        e.to_string()
    }
}

/// Column index of the first column whose key is one of `keys`.
fn col(t: &DraftTable, keys: &[&str]) -> Option<usize> {
    keys.iter().find_map(|k| t.columns.iter().position(|c| c.key == *k))
}

fn cell_amount(row: &[String], idx: Option<usize>) -> Option<f64> {
    let s = row.get(idx?)?;
    if s.trim().is_empty() {
        return None;
    }
    normalize_amount(s)
}

/// Sums of an amount column (`None` when no row carries a number).
fn column_sum(t: &DraftTable, idx: Option<usize>) -> Option<f64> {
    let idx = idx?;
    let nums: Vec<f64> = t.rows.iter().filter_map(|r| cell_amount(r, Some(idx))).collect();
    (!nums.is_empty()).then(|| nums.iter().sum())
}

/// Column sums of the item tables: (supply, tax); the first table with amounts wins.
struct TableTotals {
    supply: Option<f64>,
    tax: Option<f64>,
    table_key: String,
}

fn validate_tables(
    draft: &mut Draft,
    ko: bool,
    issues: &mut Vec<DraftIssue>,
    notes: &mut Vec<String>,
) -> Option<TableTotals> {
    let mut totals: Option<TableTotals> = None;
    for t in &mut draft.tables {
        // 수량 of goods, or 투입 MM of a staffing (software estimate) table.
        let qty = col(t, &["quantity", "qty", "months"]);
        let qty_label = qty.map(|i| t.columns[i].label.clone()).unwrap_or_else(|| "수량".into());
        let unit = col(t, &["unit_price", "price"]);
        let supply = col(t, &["supply_amount", "amount", "supply"]);
        let tax = col(t, &["tax_amount", "tax", "vat"]);
        let width = t.columns.len();
        let mut filled = 0usize;
        for (ri, row) in t.rows.iter_mut().enumerate() {
            if width > 0 && row.len() < width {
                row.resize(width, String::new());
            }
            let (q, u, s) = (cell_amount(row, qty), cell_amount(row, unit), cell_amount(row, supply));
            if let (Some(q), Some(u), Some(si)) = (q, u, supply) {
                let computed = q * u;
                match s {
                    Some(s) if !close(s, computed) => issues.push(issue(
                        format!("tables.{}[{ri}].{}", t.key, t.columns[si].key),
                        "sum_mismatch",
                        if ko {
                            format!(
                                "{}행: {} {} × 단가 {} = {} 이지만 공급가액은 {}입니다",
                                ri + 1,
                                qty_label,
                                format_number(q),
                                format_won(u),
                                format_won(computed),
                                format_won(s)
                            )
                        } else {
                            format!(
                                "row {}: qty {} × unit price {} = {} but the supply amount is {}",
                                ri + 1,
                                format_number(q),
                                format_won(u),
                                format_won(computed),
                                format_won(s)
                            )
                        },
                        true,
                        Some(format_won(computed)),
                    )),
                    Some(_) => {}
                    None => {
                        row[si] = format_won(computed);
                        filled += 1;
                    }
                }
            }
            if let (Some(ti), Some(s)) = (tax, cell_amount(row, supply)) {
                if let Some(tv) = cell_amount(row, Some(ti)) {
                    let expected = (s * 0.1).round();
                    if (tv - expected).abs() > (s * 0.005).max(10.0) {
                        issues.push(issue(
                            format!("tables.{}[{ri}].{}", t.key, t.columns[ti].key),
                            "tax_mismatch",
                            if ko {
                                format!(
                                    "{}행: 세액 {}이 공급가액의 10%({})와 다릅니다",
                                    ri + 1,
                                    format_won(tv),
                                    format_won(expected)
                                )
                            } else {
                                format!(
                                    "row {}: tax {} differs from 10% of supply ({})",
                                    ri + 1,
                                    format_won(tv),
                                    format_won(expected)
                                )
                            },
                            false,
                            Some(format_won(expected)),
                        ));
                    }
                }
            }
        }
        if filled > 0 {
            notes.push(tr(
                ko,
                "품목 표의 빈 공급가액을 수량×단가로 채웠습니다.",
                "Empty supply amounts were computed as qty × unit price.",
            ));
        }
        if totals.is_none() {
            let s = column_sum(t, supply);
            if s.is_some() {
                totals = Some(TableTotals { supply: s, tax: column_sum(t, tax), table_key: t.key.clone() });
            }
        }
    }
    totals
}

fn field_value(draft: &Draft, keys: &[&str]) -> Option<(usize, Option<f64>)> {
    keys.iter().find_map(|k| {
        draft.fields.iter().position(|f| f.key == *k).map(|i| {
            let v = draft.fields[i].value.trim();
            (i, if v.is_empty() { None } else { normalize_amount(v) })
        })
    })
}

fn set_amount(draft: &mut Draft, idx: usize, value: f64) {
    let f = &mut draft.fields[idx];
    f.value = format_won(value);
    f.normalized = Some(format_number(value));
}

/// 공급가액 합계 / 세액 / 합계 against the item table (fills empty totals).
fn validate_totals(
    draft: &mut Draft,
    table: Option<TableTotals>,
    ko: bool,
    issues: &mut Vec<DraftIssue>,
    notes: &mut Vec<String>,
) {
    let supply_f = field_value(draft, &["supply_total", "supply_amount"]);
    let tax_f = field_value(draft, &["tax_total", "tax_amount", "vat"]);
    let grand_f = field_value(draft, &["grand_total", "total", "total_amount"]);
    let mut auto = false;
    // Supply total ← Σ items.
    let mut supply: Option<f64> = supply_f.and_then(|(_, v)| v);
    if let Some(t) = &table {
        if let (Some((idx, None)), Some(sum)) = (supply_f, t.supply) {
            set_amount(draft, idx, sum);
            supply = Some(sum);
            auto = true;
        } else if let (Some((idx, Some(v))), Some(sum)) = (supply_f, t.supply) {
            if !close(v, sum) {
                issues.push(issue(
                    format!("fields.{}", draft.fields[idx].key),
                    "sum_mismatch",
                    if ko {
                        format!(
                            "공급가액 합계 {}가 품목 표의 합 {}과 다릅니다 ({})",
                            format_won(v),
                            format_won(sum),
                            t.table_key
                        )
                    } else {
                        format!(
                            "supply total {} differs from the item sum {} ({})",
                            format_won(v),
                            format_won(sum),
                            t.table_key
                        )
                    },
                    true,
                    Some(format_won(sum)),
                ));
            }
        } else if let (None, Some(sum), Some((gidx, None))) = (supply_f, t.supply, grand_f) {
            // Forms with a single total (영수증 style): Σ amount → total.
            let total = sum + t.tax.unwrap_or(0.0);
            set_amount(draft, gidx, total);
            auto = true;
        }
    }
    // Tax total ← Σ items.tax or 10 % of the supply total.
    let mut tax: Option<f64> = tax_f.and_then(|(_, v)| v);
    if let Some((idx, current)) = tax_f {
        let expected = table.as_ref().and_then(|t| t.tax).or_else(|| supply.map(|s| (s * 0.1).round()));
        match (current, expected) {
            (None, Some(e)) => {
                set_amount(draft, idx, e);
                tax = Some(e);
                auto = true;
            }
            (Some(v), Some(e)) if !close(v, e) => {
                let from_table = table.as_ref().is_some_and(|t| t.tax.is_some());
                issues.push(issue(
                    format!("fields.{}", draft.fields[idx].key),
                    if from_table { "sum_mismatch" } else { "tax_mismatch" },
                    if ko {
                        if from_table {
                            format!("세액 합계 {}가 품목 표의 세액 합 {}과 다릅니다", format_won(v), format_won(e))
                        } else {
                            format!("세액 {}이 공급가액의 10%({})와 다릅니다", format_won(v), format_won(e))
                        }
                    } else if from_table {
                        format!("tax total {} differs from the item tax sum {}", format_won(v), format_won(e))
                    } else {
                        format!("tax {} differs from 10% of the supply total ({})", format_won(v), format_won(e))
                    },
                    from_table,
                    Some(format_won(e)),
                ));
            }
            _ => {}
        }
    }
    // Grand total ← supply + tax.
    if let (Some((idx, current)), Some(s)) = (grand_f, supply) {
        let expected = s + tax.unwrap_or(0.0);
        match current {
            None => {
                set_amount(draft, idx, expected);
                auto = true;
            }
            Some(v) if !close(v, expected) => issues.push(issue(
                format!("fields.{}", draft.fields[idx].key),
                "sum_mismatch",
                if ko {
                    format!(
                        "합계 {}가 공급가액 {} + 세액 {} = {}과 다릅니다",
                        format_won(v),
                        format_won(s),
                        format_won(tax.unwrap_or(0.0)),
                        format_won(expected)
                    )
                } else {
                    format!(
                        "grand total {} differs from supply {} + tax {} = {}",
                        format_won(v),
                        format_won(s),
                        format_won(tax.unwrap_or(0.0)),
                        format_won(expected)
                    )
                },
                true,
                Some(format_won(expected)),
            )),
            Some(_) => {}
        }
    }
    if auto {
        notes.push(tr(
            ko,
            "비어 있던 합계 항목을 자동 계산해 채웠습니다.",
            "Empty total fields were computed automatically.",
        ));
    }
}

fn money_or_date_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(\d{1,3}(,\d{3})+\s*원?)|(₩\s*\d)|(\d+\s*(만|억)\s*원)|(\d{4,}\s*원)|((19|20)\d{2}\s*[.\-/년]\s*\d{1,2}\s*[.\-/월]\s*\d{1,2})|(\d{1,2}\s*월\s*\d{1,2}\s*일)",
        )
        .expect("static regex")
    })
}

/// Normalises every field by kind and reports parse / checksum / required problems.
fn validate_fields(draft: &mut Draft, spec: Option<&TemplateSpec>, ko: bool, issues: &mut Vec<DraftIssue>) {
    for f in &mut draft.fields {
        let v = f.value.trim().to_string();
        let path = format!("fields.{}", f.key);
        if v.is_empty() {
            f.normalized = None;
            if f.required {
                issues.push(issue(
                    path,
                    "required_missing",
                    if ko {
                        format!("필수 항목 '{}'이(가) 비어 있습니다", f.label)
                    } else {
                        format!("required field '{}' is empty", f.label)
                    },
                    true,
                    None,
                ));
            }
            continue;
        }
        let kind = FieldKind::parse(&f.kind).unwrap_or_default();
        f.normalized = match kind {
            FieldKind::Amount => match normalize_amount(&v) {
                Some(n) => Some(format_number(n)),
                None => {
                    issues.push(issue(
                        path.clone(),
                        "amount_invalid",
                        if ko {
                            format!("'{}'의 값 '{}'을(를) 금액으로 읽을 수 없습니다", f.label, v)
                        } else {
                            format!("'{}': '{}' is not an amount", f.label, v)
                        },
                        true,
                        None,
                    ));
                    None
                }
            },
            FieldKind::Integer => match normalize_integer(&v) {
                Some(n) => Some(n.to_string()),
                None => {
                    issues.push(issue(
                        path.clone(),
                        "integer_invalid",
                        if ko {
                            format!("'{}'의 값 '{}'은(는) 정수가 아닙니다", f.label, v)
                        } else {
                            format!("'{}': '{}' is not an integer", f.label, v)
                        },
                        false,
                        None,
                    ));
                    None
                }
            },
            FieldKind::Date => match normalize_date(&v) {
                Some(d) => Some(d),
                None => {
                    issues.push(issue(
                        path.clone(),
                        "date_invalid",
                        if ko {
                            format!("'{}'의 값 '{}'을(를) 날짜로 읽을 수 없습니다 (YYYY-MM-DD)", f.label, v)
                        } else {
                            format!("'{}': '{}' is not a recognised date (YYYY-MM-DD)", f.label, v)
                        },
                        false,
                        None,
                    ));
                    None
                }
            },
            FieldKind::Brn => match brn_digits(&v) {
                Some(d) if brn_valid(&d) => Some(format_brn(&d)),
                Some(d) => {
                    issues.push(issue(
                        path.clone(),
                        "brn_invalid",
                        if ko {
                            format!("'{}' 사업자등록번호 {}의 검증 자리가 맞지 않습니다", f.label, format_brn(&d))
                        } else {
                            format!("'{}': business registration number {} fails the checksum", f.label, format_brn(&d))
                        },
                        true,
                        None,
                    ));
                    Some(format_brn(&d))
                }
                None => {
                    issues.push(issue(
                        path.clone(),
                        "brn_invalid",
                        if ko {
                            format!("'{}' 사업자등록번호는 10자리여야 합니다: '{}'", f.label, v)
                        } else {
                            format!("'{}': a business registration number has 10 digits: '{}'", f.label, v)
                        },
                        true,
                        None,
                    ));
                    None
                }
            },
            FieldKind::Phone => normalize_phone(&v),
            FieldKind::Email => Some(v.to_ascii_lowercase()),
            _ => None,
        };
        // Keys a built-in template does not know (user-added or stale fields).
        if let Some(spec) = spec {
            if spec.field(&f.key).is_none() {
                issues.push(issue(
                    path,
                    "template_unknown_field",
                    if ko {
                        format!("'{}'은(는) {} 양식에 없는 항목입니다", f.key, spec.name(true))
                    } else {
                        format!("'{}' is not a field of the {} template", f.key, spec.name(false))
                    },
                    false,
                    None,
                ));
            }
        }
    }
    // Period order (계약기간 시작 ≤ 종료).
    let start = draft.field("period_start").and_then(|f| f.normalized.clone());
    let end = draft.field("period_end").and_then(|f| f.normalized.clone());
    if let (Some(a), Some(b)) = (start, end) {
        if a > b {
            issues.push(issue(
                "fields.period_end",
                "date_order",
                tr(ko, "계약기간 종료일이 시작일보다 앞섭니다", "the period ends before it starts"),
                true,
                None,
            ));
        }
    }
}

fn validate_body(draft: &Draft, ko: bool, issues: &mut Vec<DraftIssue>) {
    let re = money_or_date_regex();
    for (i, b) in draft.body.iter().enumerate() {
        if let DraftBlock::Paragraph { text, source: None } = b {
            if re.is_match(text) {
                issues.push(issue(
                    format!("body[{i}]"),
                    "uncited_claim",
                    if ko {
                        "금액·날짜가 포함된 문장에 출처가 없습니다 — 자료와 대조해 확인하세요".to_string()
                    } else {
                        "a sentence states money or a date without a citation — verify against the sources".to_string()
                    },
                    false,
                    None,
                ));
            }
        }
    }
}

/// Runs every check, filling `normalized`, computed totals and `issues`.
pub fn validate(draft: &mut Draft) {
    let ko = draft.language == crate::settings::Language::Ko;
    let spec = match &draft.template {
        TemplateRef::Builtin { template } => Some(TemplateSpec::builtin(*template)),
        TemplateRef::Form { .. } => None,
    };
    let mut issues = Vec::new();
    let mut notes = Vec::new();
    let totals = validate_tables(draft, ko, &mut issues, &mut notes);
    validate_totals(draft, totals, ko, &mut issues, &mut notes);
    validate_fields(draft, spec.as_ref(), ko, &mut issues);
    validate_body(draft, ko, &mut issues);
    for n in notes {
        if !draft.notes.contains(&n) {
            draft.notes.push(n);
        }
    }
    draft.issues = issues;
    draft.updated_at = Utc::now();
}
