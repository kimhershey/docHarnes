//! End-to-end tests over an in-memory workspace DB: index → hybrid search
//! (morpheme / trigram / LIKE / vector) → grounded answers, plus a small
//! Korean golden set (recall@10, top-1 clause accuracy). Runs with the
//! built-in analyzer; with `DOCULENS_KODIC` set it uses ko-dic.

use super::answer::ChatBackend;
use crate::engine::{ChatRequest, ChatResponse};
use crate::error::{CoreError, Result};
use std::collections::HashSet;

use super::answer::{answer, compare, summarize, AnswerDeps, ScriptedChat};
use super::chunker::ChunkOptions;
use super::embed::{self, Embedder, HashEmbedder};
use super::index::{index_document, index_pending, index_status, IndexDeps, IndexOutcome};
use super::repo;
use super::search::{search, SearchDeps};
use super::tokenize::{Analyzer, AnalyzerKind};
use super::*;
use crate::db::Db;
use crate::documents::{repo as docs, DocumentRole, DocumentStatus, NewDocument, NewPage, SourceKind};

const REGULATION_V2: &str = "출장비 규정 (2024년 개정)\n\n제1장 총칙\n제1조(목적) 이 규정은 임직원의 국내외 출장에 관한 사항을 정함을 목적으로 한다.\n제2조(적용 범위) 이 규정은 모든 임직원에게 적용한다.\n\n제2장 출장비\n제3조(출장비의 구성)\n① 출장비는 교통비, 숙박비, 일비로 구성한다.\n② 숙박비는 실비로 지급하되 1박당 150,000원을 한도로 한다.\n③ 일비는 1일 30,000원으로 한다.\n제4조(정산) 출장 종료 후 7일 이내에 정산서를 제출하여야 한다. 정산서에는 영수증을 첨부한다.\n제5조(해외 출장) 해외 출장의 환율은 출발일 기준 매매기준율을 적용한다.\n";
const REGULATION_V1: &str = "출장비 규정 (2023년)\n\n제1장 총칙\n제1조(목적) 이 규정은 임직원의 출장에 관한 사항을 정한다.\n\n제2장 출장비\n제3조(출장비의 구성)\n① 출장비는 교통비, 숙박비로 구성한다.\n② 숙박비는 1박당 120,000원을 한도로 한다.\n제4조(정산) 출장 종료 후 14일 이내에 정산서를 제출한다.\n";
const SECURITY: &str = "정보보안 지침\n\n제1조(목적) 이 지침은 회사 정보자산의 보호를 목적으로 한다.\n제2조(비밀번호) 비밀번호는 8자 이상으로 하고 90일마다 변경한다.\n제3조(저장매체) 외부 저장매체의 반입은 보안담당자의 승인을 받아야 한다.\n제4조(위반) 지침을 위반한 임직원은 징계할 수 있다.\n";
const CONTRACT: &str = "물품 공급 계약서\n\n제1조(계약금액) 계약금액은 금 12,000,000원(공급가액 10,909,091원, 부가세 1,090,909원)으로 한다.\n제2조(납품 기한) 을은 계약일로부터 30일 이내에 물품을 납품한다.\n제3조(지체상금) 납품이 지연되면 지체일수 1일당 계약금액의 0.1%를 지체상금으로 부과한다.\n제4조(대금 지급) 갑은 검수 완료 후 세금계산서를 받은 날부터 15일 이내에 대금을 지급한다.\n\n공급자: 주식회사 도큐 (사업자등록번호 123-45-67890)\n";
const EXPENSE_GUIDE: &str = "여비 지급 기준\n\n1. 개요\n여비는 교통비와 숙박비를 포함하며 실비 정산을 원칙으로 한다.\n2. 신청\n여비 신청서는 출장 전에 제출한다.\n";
const AGREEMENT_EN: &str = "SUPPLY AGREEMENT\n\n1. Price\nThe contract price is KRW 12,000,000 excluding VAT.\n2. Delivery\nThe supplier shall deliver the goods within 30 days of the contract date.\n3. Late penalty\nA late penalty of 0.1 percent of the contract price applies per day of delay.\n";

fn analyzer() -> Analyzer {
    Analyzer::load_first(&Analyzer::candidates(&[]))
}

fn add_doc(db: &Db, id: &str, name: &str, role: DocumentRole, pages: &[&str]) {
    db.with(|c| {
        docs::insert_document(
            c,
            &NewDocument {
                id: id.into(),
                rel_path: format!("documents/{name}"),
                original_name: name.into(),
                source_kind: SourceKind::Pdf,
                content_hash: blake3::hash(name.as_bytes()).to_hex().to_string(),
                size_bytes: 1,
                file_modified_at: None,
                role,
                import_batch_id: None,
            },
        )?;
        let new_pages: Vec<NewPage> = pages
            .iter()
            .enumerate()
            .map(|(i, t)| NewPage {
                page_no: i as u32 + 1,
                width_px: 100,
                height_px: 100,
                width_pt: None,
                height_pt: None,
                dpi: None,
                rotation: 0,
                text: if t.is_empty() { None } else { Some(t.to_string()) },
                is_blank: None,
            })
            .collect();
        docs::replace_pages(c, id, &new_pages, None, false, None)?;
        docs::set_status(c, id, DocumentStatus::Rendered, None)
    })
    .unwrap();
}

struct World {
    db: Db,
    analyzer: Analyzer,
}

impl World {
    fn new() -> World {
        let db = Db::open_in_memory().unwrap();
        add_doc(&db, "reg2", "출장비규정_2024.pdf", DocumentRole::Knowledge, &[REGULATION_V2]);
        add_doc(&db, "reg1", "출장비규정_2023.pdf", DocumentRole::Knowledge, &[REGULATION_V1]);
        add_doc(&db, "sec", "정보보안지침.pdf", DocumentRole::Knowledge, &[SECURITY]);
        add_doc(&db, "con", "공급계약서.pdf", DocumentRole::Work, &["", CONTRACT]);
        add_doc(&db, "exp", "여비지급기준.pdf", DocumentRole::Knowledge, &[EXPENSE_GUIDE]);
        add_doc(&db, "en", "supply-agreement.pdf", DocumentRole::Knowledge, &[AGREEMENT_EN]);
        add_doc(&db, "scan", "scan.pdf", DocumentRole::Work, &["", ""]);
        db.with(|c| {
            repo::set_version(
                c,
                &VersionInput {
                    document_id: "reg2".into(),
                    group_key: "출장비 규정".into(),
                    version_label: Some("2024".into()),
                    effective_date: Some("2024-01-01".into()),
                    is_latest: true,
                    note: None,
                },
            )?;
            repo::set_version(
                c,
                &VersionInput {
                    document_id: "reg1".into(),
                    group_key: "출장비 규정".into(),
                    version_label: Some("2023".into()),
                    effective_date: Some("2023-01-01".into()),
                    is_latest: false,
                    note: None,
                },
            )?;
            repo::save_glossary(
                c,
                &GlossaryInput {
                    id: None,
                    term: "출장비".into(),
                    definition: Some("출장에 드는 비용".into()),
                    synonyms: vec!["여비".into()],
                },
            )?;
            Ok(())
        })
        .unwrap();
        let w = World { db, analyzer: analyzer() };
        let report = index_pending(&w.index_deps(), None, false, None, &mut |_| {}).unwrap();
        assert_eq!(report.failed, Vec::<(String, String)>::new());
        assert_eq!(report.no_text, vec!["scan".to_string()]);
        assert_eq!(report.indexed.len(), 6);
        w
    }
    fn index_deps(&self) -> IndexDeps<'_> {
        IndexDeps {
            db: &self.db,
            analyzer: &self.analyzer,
            options: ChunkOptions { target_tokens: 120, max_tokens: 200, min_tokens: 20 },
        }
    }
    fn search_deps<'a>(&'a self, embedder: Option<&'a dyn Embedder>) -> SearchDeps<'a> {
        SearchDeps { db: &self.db, analyzer: &self.analyzer, embedder }
    }
    fn hits(&self, query: &str) -> Vec<SearchHit> {
        search(&self.search_deps(None), &SearchRequest::new(query)).unwrap().hits
    }
}

#[test]
fn indexing_is_incremental_and_tracks_text_changes() {
    let w = World::new();
    let status = index_status(&w.index_deps()).unwrap();
    assert_eq!(status.documents_total, 7);
    assert_eq!(status.documents_indexed, 7); // no-text docs count as indexed (nothing pending)
    assert_eq!(status.documents_no_text, 1);
    assert_eq!(status.documents_pending, 0);
    assert!(status.chunks_total >= 8, "{}", status.chunks_total);
    assert_eq!(status.chunks_embedded, 0);
    assert_eq!(status.knowledge_documents, 5);
    assert_eq!(status.glossary_terms, 1);
    // Second run: nothing to do.
    let report = index_pending(&w.index_deps(), None, false, None, &mut |_| {}).unwrap();
    assert!(report.indexed.is_empty() && report.unchanged == 0, "{report:?}");
    // Changed text → re-chunked (hash differs) even though indexed_at is set.
    w.db.with(|c| {
        docs::replace_pages(
            c,
            "sec",
            &[NewPage {
                page_no: 1,
                width_px: 1,
                height_px: 1,
                width_pt: None,
                height_pt: None,
                dpi: None,
                rotation: 0,
                text: Some("정보보안 지침 개정판\n제2조(비밀번호) 비밀번호는 12자 이상으로 한다.".into()),
                is_blank: None,
            }],
            None,
            false,
            None,
        )
    })
    .unwrap();
    let doc = w.db.with(|c| docs::get_document(c, "sec")).unwrap().unwrap();
    assert_eq!(index_document(&w.index_deps(), &doc, false).unwrap(), IndexOutcome::Indexed(1));
    assert_eq!(index_document(&w.index_deps(), &doc, false).unwrap(), IndexOutcome::Unchanged);
    let hits = w.hits("비밀번호 12자");
    assert_eq!(hits[0].document_id, "sec");
    let sec_chunks = w.db.with(|c| repo::list_chunks(c, "sec")).unwrap();
    assert_eq!(sec_chunks.len(), 1);
    assert!(!sec_chunks[0].text.contains("8자"), "old text gone");
    assert!(w.hits("8자 이상").iter().all(|h| !h.text.contains("8자 이상")));
    // Forced re-index keeps the chunk ids stable in count.
    let chunks = w.db.with(|c| repo::list_chunks(c, "reg2")).unwrap();
    assert!(chunks.iter().all(|c| c.heading_path.is_some()), "{chunks:#?}");
    assert!(
        chunks.iter().any(|c| c.clause.as_deref() == Some("제3조")),
        "{:?}",
        chunks.iter().map(|c| c.clause.clone()).collect::<Vec<_>>()
    );
    // Items inside 제3조 carry the 항 label when they fall into a continuation chunk.
    let narrow = ChunkOptions { target_tokens: 40, max_tokens: 60, min_tokens: 5 };
    let drafts =
        super::chunker::chunk_document(&[super::chunker::PageText { page_no: 1, text: REGULATION_V2 }], &narrow);
    assert!(
        drafts.iter().any(|d| d.clause.as_deref().is_some_and(|c| c.starts_with("제3조 제"))),
        "{:?}",
        drafts.iter().map(|d| d.clause.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn golden_set_recall_and_clause_precision() {
    let w = World::new();
    let golden: &[(&str, &str, &str)] = &[
        ("출장비는 무엇으로 구성되나?", "reg2", "제3조"),
        ("숙박비 한도", "reg2", "제3조"),
        ("정산서 제출 기한", "reg2", "제4조"),
        ("일비는 얼마인가요", "reg2", "제3조"),
        ("비밀번호 길이", "sec", "제2조"),
        ("외부 저장매체 반입", "sec", "제3조"),
        ("계약금액", "con", "제1조"),
        ("납품 기한은?", "con", "제2조"),
        ("지체상금 요율", "con", "제3조"),
        ("123-45-67890", "con", ""),
        ("공급가액은", "con", "제1조"),
        ("세금", "con", "제4조"),
        ("해외 출장 환율", "reg2", "제5조"),
        ("late penalty", "en", "Late penalty"),
        ("delivery within 30 days", "en", "Delivery"),
    ];
    let mut recall = 0;
    let mut top1 = 0;
    let mut misses = Vec::new();
    for (q, doc, clause) in golden {
        let hits = w.hits(q);
        let ids: Vec<&str> = hits.iter().take(10).map(|h| h.document_id.as_str()).collect();
        if ids.contains(doc) {
            recall += 1;
        } else {
            misses.push(format!("{q} -> {ids:?}"));
        }
        if let Some(first) = hits.first() {
            let label =
                format!("{} {}", first.clause.as_deref().unwrap_or(""), first.heading_path.as_deref().unwrap_or(""));
            if first.document_id == *doc && (clause.is_empty() || label.contains(clause)) {
                top1 += 1;
            } else {
                misses.push(format!("top1 {q} -> {} {:?}", first.document_id, first.clause));
            }
        }
    }
    let n = golden.len();
    eprintln!(
        "golden set ({:?}): recall@10 {recall}/{n}, top-1 clause {top1}/{n}\n{}",
        w.analyzer.kind(),
        misses.join("\n")
    );
    assert!(recall as f32 / n as f32 >= 0.9, "recall@10 {recall}/{n}: {misses:?}");
    let min_top1 = if w.analyzer.kind() == AnalyzerKind::KoDic { 0.75 } else { 0.6 };
    assert!(top1 as f32 / n as f32 >= min_top1, "top-1 {top1}/{n}: {misses:?}");
}

#[test]
fn korean_particles_short_words_and_numbers() {
    let w = World::new();
    // Particle-suffixed query matches the bare noun in the text.
    let hits = w.hits("숙박비는");
    assert!(hits.iter().any(|h| h.document_id == "reg2"), "{hits:#?}");
    // Two-character word → morpheme index (or LIKE fallback) rather than nothing.
    let resp = search(&w.search_deps(None), &SearchRequest::new("세금")).unwrap();
    assert!(resp.hits.iter().any(|h| h.document_id == "con"), "{resp:#?}");
    // Amount with separators (both the Korean contract and the English agreement carry it).
    let hits = w.hits("12,000,000");
    let docs_hit: HashSet<&str> = hits.iter().map(|h| h.document_id.as_str()).collect();
    assert!(docs_hit.contains("con") && docs_hit.contains("en"), "{docs_hit:?}");
    assert!(
        hits[0].highlights.iter().any(|(a, b)| hits[0]
            .snippet
            .chars()
            .skip(*a)
            .take(b - a)
            .collect::<String>()
            .contains("12")),
        "{:?}",
        hits[0]
    );
    // Highlights point at the matched words.
    let hits = w.hits("지체상금");
    let h = &hits[0];
    let (a, b) = h.highlights[0];
    assert_eq!(h.snippet.chars().skip(a).take(b - a).collect::<String>(), "지체상금");
    assert!(h.channels.contains(&SearchChannel::Morph) || h.channels.contains(&SearchChannel::Trigram));
}

#[test]
fn glossary_synonyms_expand_queries() {
    let w = World::new();
    let resp = search(&w.search_deps(None), &SearchRequest::new("출장비 신청")).unwrap();
    assert!(resp.expanded_terms.contains(&"여비".to_string()), "{:?}", resp.expanded_terms);
    assert!(
        resp.hits.iter().any(|h| h.document_id == "exp"),
        "{:#?}",
        resp.hits.iter().map(|h| &h.document_id).collect::<Vec<_>>()
    );
    let mut req = SearchRequest::new("출장비 신청");
    req.expand_glossary = false;
    let resp = search(&w.search_deps(None), &req).unwrap();
    assert!(resp.expanded_terms.is_empty());
}

#[test]
fn scopes_versions_and_collections() {
    let w = World::new();
    // latest_only hides the 2023 edition.
    let hits = w.hits("숙박비 한도");
    assert!(
        hits.iter().all(|h| h.document_id != "reg1"),
        "{:?}",
        hits.iter().map(|h| &h.document_id).collect::<Vec<_>>()
    );
    let mut req = SearchRequest::new("숙박비 한도");
    req.scope.latest_only = false;
    let hits = search(&w.search_deps(None), &req).unwrap().hits;
    assert!(hits.iter().any(|h| h.document_id == "reg1"));
    // Role scope.
    let mut req = SearchRequest::new("계약금액");
    req.scope = SearchScope::knowledge();
    assert!(search(&w.search_deps(None), &req).unwrap().hits.iter().all(|h| h.document_id != "con"));
    // Collection scope + inactive collections.
    let col =
        w.db.with(|c| repo::save_collection(c, &CollectionInput { name: "보안".into(), ..CollectionInput::default() }))
            .unwrap();
    w.db.with(|c| repo::add_docs_to_collection(c, &col.id, &["sec".into()])).unwrap();
    let mut req = SearchRequest::new("제1조 목적");
    req.scope.collections = vec![col.id.clone()];
    let hits = search(&w.search_deps(None), &req).unwrap().hits;
    assert!(
        !hits.is_empty() && hits.iter().all(|h| h.document_id == "sec"),
        "{:?}",
        hits.iter().map(|h| &h.document_id).collect::<Vec<_>>()
    );
    let inactive =
        w.db.with(|c| {
            repo::save_collection(
                c,
                &CollectionInput {
                    id: Some(col.id.clone()),
                    name: "보안".into(),
                    active: false,
                    ..CollectionInput::default()
                },
            )
        })
        .unwrap();
    assert!(!inactive.active);
    assert!(w.hits("비밀번호").iter().all(|h| h.document_id != "sec"));
    let cols = w.db.with(repo::list_collections).unwrap();
    assert_eq!((cols.len(), cols[0].doc_count), (1, 1));
    assert!(cols[0].chunk_count >= 1);
    // Document scope + folder scope.
    let mut req = SearchRequest::new("제1조");
    req.scope.documents = vec!["con".into()];
    assert!(search(&w.search_deps(None), &req).unwrap().hits.iter().all(|h| h.document_id == "con"));
    let mut req = SearchRequest::new("제1조");
    req.scope.folder = Some("documents".into());
    assert!(!search(&w.search_deps(None), &req).unwrap().hits.is_empty());
    req.scope.folder = Some("inbox".into());
    assert!(search(&w.search_deps(None), &req).unwrap().hits.is_empty());
    // Versions listing.
    let groups = w.db.with(repo::list_version_groups).unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].documents.iter().filter(|d| d.is_latest).count(), 1);
    assert_eq!(groups[0].documents[0].document_id, "reg2");
}

/// 0.5.0 plan A: a project scope = its work documents ∪ its reference group ∪ the active groups
/// of the 공통 참고 자료; the group is created on first use, follows a rename and goes with the
/// project.
#[test]
fn project_scope_unions_work_documents_and_reference_material() {
    use crate::projects::{repo as projects, ProjectInput};
    let w = World::new();
    let sorted = |set: HashSet<String>| {
        let mut v: Vec<String> = set.into_iter().collect();
        v.sort_unstable();
        v
    };
    // Work document: the contract. Reference material: the security guideline. Common groups:
    // the 2024 regulation (active) and the expense guide (inactive); the English agreement is
    // in no group.
    let project =
        w.db.with(|c| projects::save_project(c, &ProjectInput { name: "한빛 납품".into(), ..Default::default() }))
            .unwrap();
    w.db.with(|c| projects::add_documents(c, &project.id, &["con".into()])).unwrap();
    let group = w.db.with(|c| repo::project_reference_group(c, &project.id, &project.name)).unwrap();
    assert_eq!(
        (group.kind, group.project_id.as_deref(), group.name.as_str(), group.active),
        (CollectionKind::Project, Some(project.id.as_str()), "한빛 납품", true)
    );
    assert_eq!(w.db.with(|c| repo::project_reference_group(c, &project.id, "x")).unwrap().id, group.id, "once");
    assert_eq!(
        w.db.with(|c| repo::reference_group_of_project(c, &project.id)).unwrap().map(|g| g.id),
        Some(group.id.clone())
    );
    assert!(w
        .db
        .with(|c| repo::project_reference_group(c, "nope", "x"))
        .unwrap_err()
        .to_string()
        .contains("project_not_found"));
    w.db.with(|c| repo::add_docs_to_collection(c, &group.id, &["sec".into()])).unwrap();
    let common =
        w.db.with(|c| {
            repo::save_collection(
                c,
                &CollectionInput { name: "사규".into(), active: true, ..CollectionInput::default() },
            )
        })
        .unwrap();
    assert!(common.is_common() && common.kind == CollectionKind::Reference);
    w.db.with(|c| repo::add_docs_to_collection(c, &common.id, &["reg2".into()])).unwrap();
    let off =
        w.db.with(|c| {
            repo::save_collection(
                c,
                &CollectionInput { name: "보류".into(), active: false, ..CollectionInput::default() },
            )
        })
        .unwrap();
    w.db.with(|c| repo::add_docs_to_collection(c, &off.id, &["exp".into()])).unwrap();
    // A project's group carries the project's name even when a common group has it (and the
    // other way round); common groups stay unique among themselves; one group per project.
    let p2 =
        w.db.with(|c| projects::save_project(c, &ProjectInput { name: "사규".into(), ..Default::default() })).unwrap();
    let g2 = w.db.with(|c| repo::project_reference_group(c, &p2.id, &p2.name)).unwrap();
    assert_eq!(
        (g2.name.as_str(), g2.kind, g2.project_id.as_deref()),
        ("사규", CollectionKind::Project, Some(p2.id.as_str()))
    );
    let same_as_project =
        w.db.with(|c| {
            repo::save_collection(
                c,
                &CollectionInput { name: "한빛 납품".into(), active: true, ..CollectionInput::default() },
            )
        })
        .unwrap();
    assert!(same_as_project.is_common());
    let dup =
        w.db.with(|c| repo::save_collection(c, &CollectionInput { name: "사규".into(), ..CollectionInput::default() }))
            .unwrap_err();
    assert!(dup.to_string().contains("collection_name_taken"), "{dup}");
    let second =
        w.db.with(|c| {
            repo::save_collection(
                c,
                &CollectionInput {
                    name: "임시".into(), project_id: Some(p2.id.clone()), ..CollectionInput::default()
                },
            )
        })
        .unwrap_err();
    assert!(second.to_string().contains("project_group_exists"), "{second}");
    let p3 =
        w.db.with(|c| projects::save_project(c, &ProjectInput { name: "세 번째".into(), ..Default::default() }))
            .unwrap();
    let via_input =
        w.db.with(|c| {
            repo::save_collection(
                c,
                &CollectionInput {
                    name: "임시".into(), project_id: Some(p3.id.clone()), ..CollectionInput::default()
                },
            )
        })
        .unwrap();
    assert_eq!((via_input.kind, via_input.project_id.as_deref()), (CollectionKind::Project, Some(p3.id.as_str())));
    assert!(w.db.with(|c| projects::delete_project(c, &p3.id)).unwrap());
    assert!(w.db.with(|c| repo::get_collection(c, &via_input.id)).unwrap().is_none());

    let allowed = w.db.with(|c| repo::scope_doc_ids(c, &SearchScope::project(&project.id))).unwrap().unwrap();
    assert_eq!(sorted(allowed), vec!["con", "reg2", "sec"]);
    let mut req = SearchRequest::new("계약금액");
    req.scope = SearchScope::project(&project.id);
    let hits = search(&w.search_deps(None), &req).unwrap().hits;
    assert!(
        !hits.is_empty() && hits.iter().all(|h| ["con", "reg2", "sec"].contains(&h.document_id.as_str())),
        "{:?}",
        hits.iter().map(|h| &h.document_id).collect::<Vec<_>>()
    );
    let mut req = SearchRequest::new("late penalty per day of delay");
    req.scope = SearchScope::project(&project.id);
    assert!(search(&w.search_deps(None), &req).unwrap().hits.iter().all(|h| h.document_id != "en"));
    // Explicit documents stand in for the work documents; the reference material stays.
    let mut scope = SearchScope::project(&project.id);
    scope.documents = vec!["en".into()];
    let allowed = w.db.with(|c| repo::scope_doc_ids(c, &scope)).unwrap().unwrap();
    assert_eq!(sorted(allowed), vec!["en", "reg2", "sec"]);
    // The other filters still intersect.
    let mut scope = SearchScope::project(&project.id);
    scope.roles = vec![DocumentRole::Work];
    assert_eq!(sorted(w.db.with(|c| repo::scope_doc_ids(c, &scope)).unwrap().unwrap()), vec!["con"]);
    // The project's own material is wanted even when superseded (reg1) or in an inactive group (exp).
    w.db.with(|c| repo::add_docs_to_collection(c, &group.id, &["reg1".into(), "exp".into()])).unwrap();
    let allowed = w.db.with(|c| repo::scope_doc_ids(c, &SearchScope::project(&project.id))).unwrap().unwrap();
    assert_eq!(sorted(allowed), vec!["con", "exp", "reg1", "reg2", "sec"]);
    assert!(w.db.with(|c| repo::scope_doc_ids(c, &SearchScope::project("nope"))).unwrap().unwrap().is_empty());
    assert!(!SearchScope::project(&project.id).is_unrestricted());
    // The group follows a rename and goes with the project; the documents stay.
    w.db.with(|c| {
        projects::save_project(
            c,
            &ProjectInput { id: Some(project.id.clone()), name: "한빛 납품 2차".into(), ..Default::default() },
        )
    })
    .unwrap();
    assert_eq!(w.db.with(|c| repo::get_collection(c, &group.id)).unwrap().unwrap().name, "한빛 납품 2차");
    assert!(w.db.with(|c| projects::delete_project(c, &project.id)).unwrap());
    assert!(w.db.with(|c| repo::get_collection(c, &group.id)).unwrap().is_none());
    assert!(w.db.with(|c| docs::get_document(c, "sec")).unwrap().is_some());
    let cols = w.db.with(repo::list_collections).unwrap();
    assert_eq!(cols.iter().filter(|c| c.is_common()).count(), 3);
    assert_eq!(cols.iter().filter(|c| c.project_id.as_deref() == Some(p2.id.as_str())).count(), 1);
}

#[test]
fn embeddings_vector_search_and_model_changes() {
    let w = World::new();
    let embedder = HashEmbedder::new(32);
    let report = embed::embed_pending(&w.db, &embedder, 4, None, &mut |_, _| {}).unwrap();
    let (total, embedded) = w.db.with(repo::chunk_counts).unwrap();
    assert_eq!((report.embedded, report.remaining, report.failed), (total, 0, 0));
    assert_eq!(embedded, total);
    assert_eq!(report.dim, Some(32));
    let info = w.db.with(embed::embedding_info).unwrap();
    assert_eq!((info.model.as_deref(), info.dim), (Some("hash-32"), Some(32)));
    // The chunk's own text is its nearest neighbour.
    let chunk =
        w.db.with(|c| repo::list_chunks(c, "con")).unwrap().into_iter().find(|c| c.text.contains("지체상금")).unwrap();
    let q =
        embed::embed_query(&embedder, &super::index::indexed_text(chunk.heading_path.as_deref(), &chunk.text)).unwrap();
    let nn = w.db.with(|c| embed::knn(c, &q, 3, None)).unwrap();
    assert_eq!(nn[0].0, chunk.id);
    assert!(nn[0].1 < 0.01, "{:?}", nn[0]);
    // Scoped KNN only returns allowed documents.
    let allowed: HashSet<String> = ["sec".to_string()].into_iter().collect();
    let nn = w.db.with(|c| embed::knn(c, &q, 3, Some(&allowed))).unwrap();
    let ids: Vec<i64> = nn.iter().map(|(id, _)| *id).collect();
    let rows = w.db.with(|c| repo::chunk_rows(c, &ids)).unwrap();
    assert!(!rows.is_empty() && rows.iter().all(|r| r.chunk.document_id == "sec"));
    // Semantic + hybrid modes report vector use and similarity.
    let mut req = SearchRequest::new("지체상금 부과");
    req.mode = SearchMode::Semantic;
    let resp = search(&w.search_deps(Some(&embedder)), &req).unwrap();
    assert!(resp.used_vector);
    assert!(resp.hits.iter().all(|h| h.channels == vec![SearchChannel::Vector] && h.vector_similarity.is_some()));
    assert_eq!(resp.hits[0].chunk_id, chunk.id);
    req.mode = SearchMode::Hybrid;
    let resp = search(&w.search_deps(Some(&embedder)), &req).unwrap();
    assert!(resp.used_vector && resp.hits[0].document_id == "con");
    // A different model/dimension rebuilds the table.
    let other = HashEmbedder::new(16);
    let report = embed::embed_pending(&w.db, &other, 50, None, &mut |_, _| {}).unwrap();
    assert!(report.reset);
    assert_eq!(report.dim, Some(16));
    let (_, embedded) = w.db.with(repo::chunk_counts).unwrap();
    assert_eq!(embedded, total);
    // Deleting a document removes its vectors.
    w.db.with(|c| docs::delete_document(c, "con")).unwrap();
    let n: i64 = w.db.with(|c| c.query_row("SELECT COUNT(*) FROM chunks_vec", [], |r| r.get(0))).unwrap();
    let (total2, _) = w.db.with(repo::chunk_counts).unwrap();
    assert_eq!(n as u64, total2);
    assert!(total2 < total);
    assert!(w.hits("지체상금").is_empty());
    // Reset drops everything.
    w.db.with(embed::reset_embeddings).unwrap();
    assert!(!w.db.with(repo::vec_table_exists).unwrap());
    assert_eq!(w.db.with(embed::embedding_info).unwrap().chunks_embedded, 0);
}

#[test]
fn grounded_answers_cite_sources_and_handle_no_evidence() {
    let w = World::new();
    let chat = ScriptedChat {
        reply:
            "출장비는 교통비, 숙박비, 일비로 구성됩니다[1]. 숙박비 한도는 1박당 150,000원입니다 [1][2].\n\n상충: 없음"
                .into(),
        model: "qwen-test".into(),
    };
    let deps =
        AnswerDeps { search: w.search_deps(None), chat: &chat, model: None, cancelled: None, context_size: None };
    let mut streamed = String::new();
    let a = answer(&deps, &AnswerRequest::question("출장비는 무엇으로 구성되나요?"), &mut |d| {
        streamed.push_str(d);
        true
    })
    .unwrap();
    assert_eq!(streamed, chat.reply);
    assert!(!a.no_evidence && !a.used_general_knowledge);
    assert!(!a.sources.is_empty() && a.sources[0].document_id == "reg2", "{:?}", a.sources);
    assert_eq!(a.sentences.len(), 3, "{:#?}", a.sentences);
    assert_eq!(a.sentences[0].citations, vec![1]);
    assert_eq!(
        a.sentences[1].citations,
        vec![1, 2].into_iter().filter(|n| *n as usize <= a.sources.len()).collect::<Vec<_>>()
    );
    assert_eq!(a.conflicts, None, "\"상충: 없음\" is not a conflict");
    assert_eq!(a.model.as_deref(), Some("qwen-test"));
    let id = a.id.expect("saved");
    let saved = w.db.with(|c| repo::list_answers(c, Some(AnswerKind::Answer), None, 10)).unwrap();
    assert_eq!(saved[0].id, id);
    assert_eq!(saved[0].sources.len(), a.sources.len());
    assert!(w.db.with(|c| repo::set_answer_feedback(c, id, Some(-1), Some("한도가 틀림"))).unwrap());
    let st = w.db.with(repo::index_status).unwrap();
    assert_eq!((st.answers, st.answers_wrong), (1, 1));

    // No evidence: nonsense query → canned text, no model call.
    let silent = ScriptedChat { reply: "SHOULD NOT BE CALLED".into(), model: "m".into() };
    let deps =
        AnswerDeps { search: w.search_deps(None), chat: &silent, model: None, cancelled: None, context_size: None };
    let a = answer(&deps, &AnswerRequest::question("플루토늄 원자로 냉각수 xyzzy"), &mut |_| true).unwrap();
    assert!(a.no_evidence && a.sources.is_empty());
    assert_eq!(a.text, super::answer::no_evidence_text(true));
    // … unless general knowledge is allowed.
    let mut req = AnswerRequest::question("플루토늄 원자로 냉각수 xyzzy");
    req.allow_general = true;
    let a = answer(&deps, &req, &mut |_| true).unwrap();
    assert!(a.no_evidence && a.used_general_knowledge && a.text == "SHOULD NOT BE CALLED");
    // English question keeps English no-evidence text.
    let a = answer(&deps, &AnswerRequest::question("plutonium reactor coolant xyzzy"), &mut |_| true).unwrap();
    assert_eq!(a.text, super::answer::no_evidence_text(false));

    // Explain: selection drives retrieval; follow-up keeps chunks.
    let chat = ScriptedChat {
        reply: "지체상금은 납품 지연 시 부과되는 금액입니다[1].".into(), model: "m".into()
    };
    let deps =
        AnswerDeps { search: w.search_deps(None), chat: &chat, model: None, cancelled: None, context_size: None };
    let mut req = AnswerRequest::question("");
    req.kind = AnswerKind::Explain;
    req.selection = Some("지체일수 1일당 계약금액의 0.1%".into());
    req.document_id = Some("con".into());
    let a = answer(&deps, &req, &mut |_| true).unwrap();
    assert_eq!(a.kind, AnswerKind::Explain);
    assert_eq!(a.sources[0].document_id, "con");
    assert_eq!(a.document_id.as_deref(), Some("con"));
    let keep = vec![a.sources[0].chunk_id];
    let mut follow = AnswerRequest::question("그럼 상한은?");
    follow.keep_chunks = keep.clone();
    let a2 = answer(&deps, &follow, &mut |_| true).unwrap();
    assert_eq!(a2.sources[0].chunk_id, keep[0]);

    // Missing model → invalid error, nothing saved.
    struct NoModel;
    impl super::answer::ChatBackend for NoModel {
        fn model_id(&self) -> Option<String> {
            None
        }
        fn chat_stream(
            &self,
            _r: &crate::engine::ChatRequest,
            _d: &mut dyn FnMut(&str) -> bool,
        ) -> crate::error::Result<crate::engine::ChatResponse> {
            unreachable!()
        }
    }
    let deps =
        AnswerDeps { search: w.search_deps(None), chat: &NoModel, model: None, cancelled: None, context_size: None };
    let err = answer(&deps, &AnswerRequest::question("출장비"), &mut |_| true).unwrap_err();
    assert!(err.to_string().contains("no_active_model"), "{err}");
}

#[test]
fn summaries_map_reduce_and_comparisons() {
    let w = World::new();
    let chat = ScriptedChat {
        reply: "- 출장비는 교통비·숙박비·일비로 구성된다[1].\n- 숙박비 한도는 150,000원이다[2].".into(),
        model: "m".into(),
    };
    let deps =
        AnswerDeps { search: w.search_deps(None), chat: &chat, model: None, cancelled: None, context_size: None };
    let mut req = AnswerRequest::question("");
    req.kind = AnswerKind::Summary;
    req.document_id = Some("reg2".into());
    req.context_tokens = 150; // forces several groups → map-reduce
    let a = summarize(&deps, &req, &mut |_| true).unwrap();
    assert_eq!(a.kind, AnswerKind::Summary);
    let chunks = w.db.with(|c| repo::list_chunks(c, "reg2")).unwrap();
    assert_eq!(a.sources.len(), chunks.len());
    assert_eq!(a.sentences.len(), 2);
    assert_eq!(a.sentences[1].citations, vec![2]);
    assert!(a.id.is_some());
    assert_eq!(w.db.with(|c| repo::list_answers(c, Some(AnswerKind::Summary), Some("reg2"), 5)).unwrap().len(), 1);
    // No text → not found.
    req.document_id = Some("scan".into());
    assert!(summarize(&deps, &req, &mut |_| true).is_err());
    // Compare two editions.
    let mut req = AnswerRequest::question("숙박비 한도");
    req.kind = AnswerKind::Compare;
    req.document_id = Some("reg2".into());
    req.compare_document_id = Some("reg1".into());
    let a = compare(&deps, &req, &mut |_| true).unwrap();
    assert_eq!(a.kind, AnswerKind::Compare);
    let docs_cited: HashSet<&str> = a.sources.iter().map(|s| s.document_id.as_str()).collect();
    assert!(docs_cited.contains("reg2") && docs_cited.contains("reg1"), "{docs_cited:?}");
}

/// Replies like `ScriptedChat` but refuses prompts above `limit` estimated tokens the way llama-server
/// does (HTTP 400 "exceeds the available context size"), recording every attempt's size.
struct LimitedChat {
    limit: u32,
    reply: String,
    sizes: std::sync::Mutex<Vec<u32>>,
}

impl LimitedChat {
    fn new(limit: u32, reply: &str) -> LimitedChat {
        LimitedChat { limit, reply: reply.into(), sizes: std::sync::Mutex::new(Vec::new()) }
    }
    fn sizes(&self) -> Vec<u32> {
        self.sizes.lock().unwrap().clone()
    }
}

impl ChatBackend for LimitedChat {
    fn model_id(&self) -> Option<String> {
        Some("limited".into())
    }
    fn chat_stream(&self, req: &ChatRequest, on_delta: &mut dyn FnMut(&str) -> bool) -> Result<ChatResponse> {
        let size: u32 = req
            .messages
            .iter()
            .map(|m| crate::planner::prompts::approx_tokens(m["content"].as_str().unwrap_or("")))
            .sum();
        self.sizes.lock().unwrap().push(size);
        if size > self.limit {
            return Err(CoreError::engine(format!(
                "/v1/chat/completions: HTTP 400: request ({size} tokens) exceeds the available context size ({} tokens), try increasing it",
                self.limit
            )));
        }
        on_delta(&self.reply);
        Ok(ChatResponse { content: self.reply.clone(), ..ChatResponse::default() })
    }
    fn complete(&self, req: &ChatRequest) -> Result<ChatResponse> {
        self.chat_stream(req, &mut |_| true)
    }
}

/// A ten-page regulation (the fixture's regulations are a few hundred tokens each, too short for the
/// context-budget tests): every page carries the regulation plus its own appendix line.
fn add_long_regulation(w: &World) {
    let pages: Vec<String> = (1..=10)
        .map(|i| format!("{REGULATION_V2}\n\n부록 {i}. 제{i}장 보칙 — 본 장은 {i}번째 개정 사항을 담는다."))
        .collect();
    let refs: Vec<&str> = pages.iter().map(String::as_str).collect();
    add_doc(&w.db, "big", "출장비규정_전문.pdf", DocumentRole::Knowledge, &refs);
    let report = index_pending(&w.index_deps(), None, false, None, &mut |_| {}).unwrap();
    assert_eq!(report.indexed.len(), 1, "{report:?}");
}

fn summary_request() -> AnswerRequest {
    let mut req = AnswerRequest::question("");
    req.kind = AnswerKind::Summary;
    req.document_id = Some("big".into());
    req.context_tokens = 24_000; // the setting is far above any real slot
    req
}

#[test]
fn summaries_fit_the_model_context_and_split_when_the_server_still_refuses() {
    let w = World::new();
    add_long_regulation(&w);
    // Reference: no limit, context unknown → the whole document in as few groups as the setting allows.
    let free = LimitedChat::new(u32::MAX, "- 핵심 [1]");
    let deps =
        AnswerDeps { search: w.search_deps(None), chat: &free, model: None, cancelled: None, context_size: None };
    summarize(&deps, &summary_request(), &mut |_| true).unwrap();
    let full = *free.sizes().iter().max().unwrap();
    let free_calls = free.sizes().len();

    // 1) A known (small) model context caps the evidence budget: more, smaller requests, each below the slot.
    let slot = 2_600;
    assert!(full > slot, "the long document must not fit a 2.6K slot in one group: {full}");
    let capped = LimitedChat::new(slot, "- 핵심 [1]");
    let deps = AnswerDeps {
        search: w.search_deps(None),
        chat: &capped,
        model: None,
        cancelled: None,
        context_size: Some(slot),
    };
    let a = summarize(&deps, &summary_request(), &mut |_| true).unwrap();
    assert!(!a.text.is_empty());
    let sizes = capped.sizes();
    assert!(sizes.iter().all(|s| *s <= slot), "every request fits the slot: {sizes:?} (slot {slot})");
    assert!(sizes.len() > free_calls, "{} vs {free_calls}", sizes.len());

    // 2) Context unknown and the server refuses the first group anyway (its tokenizer counts more than
    //    the estimate): the group is split and retried instead of failing the whole summary.
    let strict = LimitedChat::new(full - 1, "- 핵심 [1]");
    let deps =
        AnswerDeps { search: w.search_deps(None), chat: &strict, model: None, cancelled: None, context_size: None };
    let a = summarize(&deps, &summary_request(), &mut |_| true).unwrap();
    assert!(!a.text.is_empty());
    let sizes = strict.sizes();
    assert!(sizes[0] >= full && sizes.len() >= 2 && *sizes.last().unwrap() < full, "{sizes:?}");
    let chunks = w.db.with(|c| repo::list_chunks(c, "big")).unwrap();
    assert_eq!(a.sources.len(), chunks.len(), "every chunk still reaches the summary");
}

#[test]
fn an_answer_prompt_the_server_refuses_is_retried_with_fewer_sources() {
    let w = World::new();
    add_long_regulation(&w);
    let question = "출장비는 무엇으로 구성되나요?";
    let free = LimitedChat::new(u32::MAX, "출장비는 교통비·숙박비·일비로 구성됩니다[1].");
    let deps =
        AnswerDeps { search: w.search_deps(None), chat: &free, model: None, cancelled: None, context_size: None };
    let mut req = AnswerRequest::question(question);
    req.top_k = 8;
    req.context_tokens = 24_000;
    let full_answer = answer(&deps, &req, &mut |_| true).unwrap();
    assert!(full_answer.sources.len() >= 2, "the fixture must yield several sources: {:?}", full_answer.sources.len());
    let full = free.sizes()[0];

    let strict = LimitedChat::new(full - 1, "출장비는 교통비·숙박비·일비로 구성됩니다[1].");
    let deps =
        AnswerDeps { search: w.search_deps(None), chat: &strict, model: None, cancelled: None, context_size: None };
    let a = answer(&deps, &req, &mut |_| true).unwrap();
    let sizes = strict.sizes();
    assert!(sizes.len() >= 2 && sizes[0] >= full && *sizes.last().unwrap() < full, "{sizes:?}");
    assert!(!a.sources.is_empty() && a.sources.len() < full_answer.sources.len(), "{}", a.sources.len());
    assert!(!a.no_evidence);
}

#[test]
fn a_text_only_request_skips_retrieval_and_cites_nothing() {
    let w = World::new();
    let chat = ScriptedChat {
        reply: "금번 과업은 당사가 수행하며, 기한은 12월 말까지입니다.".into(),
        model: "m".into(),
    };
    let deps =
        AnswerDeps { search: w.search_deps(None), chat: &chat, model: None, cancelled: None, context_size: None };
    let mut req = AnswerRequest::question(
        "다음 문장을 다듬어줘: 금번 과업은 당사가 수행하며 기한은 12월말까지로 함. 출장비 규정은 참고만.",
    );
    req.retrieve = false;
    let a = answer(&deps, &req, &mut |_| true).unwrap();
    assert!(a.sources.is_empty() && !a.no_evidence && !a.used_general_knowledge, "{a:?}");
    assert_eq!(a.text, chat.reply);
    assert_eq!(a.sentences.len(), 1);
    // The same sentence with retrieval on would have looked the regulations up (출장비) or reported no evidence.
    req.retrieve = true;
    let b = answer(&deps, &req, &mut |_| true).unwrap();
    assert!(!b.sources.is_empty() || b.no_evidence);
}

#[test]
fn collections_glossary_and_version_crud() {
    let db = Db::open_in_memory().unwrap();
    add_doc(&db, "d1", "a.pdf", DocumentRole::Knowledge, &["제1조 하나"]);
    add_doc(&db, "d2", "b.pdf", DocumentRole::Knowledge, &["제1조 둘"]);
    let c = db
        .with(|c| {
            repo::save_collection(
                c,
                &CollectionInput {
                    name: " 규정 ".into(),
                    description: Some("사내 규정".into()),
                    ..CollectionInput::default()
                },
            )
        })
        .unwrap();
    assert_eq!(c.name, "규정");
    assert!(
        db.with(|x| repo::save_collection(x, &CollectionInput { name: "규정".into(), ..CollectionInput::default() }))
            .is_err(),
        "duplicate name"
    );
    assert!(db
        .with(|x| repo::save_collection(x, &CollectionInput { name: "  ".into(), ..CollectionInput::default() }))
        .is_err());
    assert_eq!(
        db.with(|x| repo::add_docs_to_collection(x, &c.id, &["d1".into(), "d2".into(), "d1".into()])).unwrap(),
        2
    );
    assert_eq!(db.with(|x| repo::collection_doc_ids(x, &c.id)).unwrap().len(), 2);
    assert_eq!(db.with(|x| repo::collections_of_document(x, "d1")).unwrap(), vec![(c.id.clone(), "규정".to_string())]);
    assert_eq!(db.with(|x| repo::remove_docs_from_collection(x, &c.id, &["d2".into()])).unwrap(), 1);
    assert!(db.with(|x| repo::delete_collection(x, &c.id)).unwrap());
    assert!(db.with(|x| repo::collections_of_document(x, "d1")).unwrap().is_empty());

    let g = db
        .with(|x| {
            repo::save_glossary(
                x,
                &GlossaryInput {
                    id: None,
                    term: "VAT".into(),
                    definition: Some("부가가치세".into()),
                    synonyms: vec!["부가세".into(), " 부가세 ".into(), "VAT".into()],
                },
            )
        })
        .unwrap();
    assert_eq!(g.synonyms, vec!["부가세".to_string()]);
    let g2 = db
        .with(|x| {
            repo::save_glossary(
                x,
                &GlossaryInput { id: None, term: "vat".into(), definition: None, synonyms: vec!["세금".into()] },
            )
        })
        .unwrap();
    assert_eq!(g2.id, g.id, "case-insensitive upsert");
    let words = |s: &str| s.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>();
    let exp =
        db.with(|x| repo::glossary_expansions(x, "세금 없음", &["세금".into(), "없음".into()], &words)).unwrap();
    assert_eq!(exp, vec![("세금".to_string(), vec!["VAT".to_string()])], "term keeps its original spelling");
    assert!(db.with(|x| repo::delete_glossary(x, g.id)).unwrap());

    let v = db
        .with(|x| {
            repo::set_version(
                x,
                &VersionInput {
                    document_id: "d1".into(),
                    group_key: "규정".into(),
                    version_label: Some("v1".into()),
                    effective_date: None,
                    is_latest: true,
                    note: None,
                },
            )
        })
        .unwrap();
    assert!(v.is_latest);
    let v2 = db
        .with(|x| {
            repo::set_version(
                x,
                &VersionInput {
                    document_id: "d2".into(),
                    group_key: "규정".into(),
                    version_label: Some("v2".into()),
                    effective_date: None,
                    is_latest: true,
                    note: None,
                },
            )
        })
        .unwrap();
    assert!(v2.is_latest);
    assert!(!db.with(|x| repo::get_version(x, "d1")).unwrap().unwrap().is_latest, "demoted");
    assert!(db.with(|x| repo::clear_version(x, "d1")).unwrap());
    assert_eq!(db.with(repo::list_version_groups).unwrap()[0].documents.len(), 1);
    assert!(db
        .with(|x| repo::set_version(
            x,
            &VersionInput { document_id: "d1".into(), group_key: " ".into(), ..VersionInput::default() }
        ))
        .is_err());
}

#[test]
fn scoped_search_takes_the_top_n_inside_the_scope() {
    // 120 filler documents outrank the target on BM25 for "제1조"; with limit 1 the
    // global candidate window (50) would never reach it unless the scope is in the SQL.
    let db = Db::open_in_memory().unwrap();
    for i in 0..120 {
        let text = format!("제1조 목적 {}\n{}", i, "제1조 제1조 제1조 제1조 목적 목적 목적 ".repeat(3));
        add_doc(&db, &format!("f{i}"), &format!("filler{i}.pdf"), DocumentRole::Knowledge, &[&text]);
    }
    add_doc(&db, "target", "target.pdf", DocumentRole::Knowledge, &["제1조 목적 이 규정은 대상 문서를 위한 것이다."]);
    let analyzer = analyzer();
    let deps = IndexDeps { db: &db, analyzer: &analyzer, options: ChunkOptions::default() };
    index_pending(&deps, None, false, None, &mut |_| {}).unwrap();
    let mut req = SearchRequest::new("제1조 목적");
    req.limit = 1;
    req.scope.documents = vec!["target".into()];
    let resp = search(&SearchDeps { db: &db, analyzer: &analyzer, embedder: None }, &req).unwrap();
    assert_eq!(resp.hits.len(), 1, "{resp:?}");
    assert_eq!(resp.hits[0].document_id, "target");
    // Vector channel with a scope: the KNN is also scoped (or widened) to the document.
    let embedder = HashEmbedder::new(16);
    embed::embed_pending(&db, &embedder, 64, None, &mut |_, _| {}).unwrap();
    req.mode = SearchMode::Semantic;
    let resp = search(&SearchDeps { db: &db, analyzer: &analyzer, embedder: Some(&embedder) }, &req).unwrap();
    assert!(resp.used_vector);
    assert!(resp.hits.iter().all(|h| h.document_id == "target"), "{resp:?}");
}

#[test]
fn explain_from_a_superseded_edition_still_cites_that_document() {
    let w = World::new();
    let chat = ScriptedChat { reply: "구판의 한도는 120,000원입니다[1].".into(), model: "m".into() };
    let deps =
        AnswerDeps { search: w.search_deps(None), chat: &chat, model: None, cancelled: None, context_size: None };
    let mut req = AnswerRequest::question("");
    req.kind = AnswerKind::Explain;
    req.selection = Some("숙박비는 1박당 120,000원을 한도로 한다".into());
    req.document_id = Some("reg1".into()); // is_latest = 0
    let a = answer(&deps, &req, &mut |_| true).unwrap();
    assert!(
        a.sources.iter().any(|c| c.document_id == "reg1"),
        "{:?}",
        a.sources.iter().map(|c| &c.document_id).collect::<Vec<_>>()
    );
    // Explicit document scopes ignore latest_only / inactive-collection exclusions.
    let mut sreq = SearchRequest::new("숙박비 한도");
    sreq.scope.documents = vec!["reg1".into()];
    let hits = search(&w.search_deps(None), &sreq).unwrap().hits;
    assert!(hits.iter().any(|h| h.document_id == "reg1"));
}

#[test]
fn metadata_updates_do_not_requeue_and_a_new_analyzer_signature_reindexes_everything() {
    let w = World::new();
    // A role change bumps updated_at → the document would be re-read; the run touches indexed_at instead.
    w.db.with(|c| docs::set_role(c, "sec", DocumentRole::Knowledge)).unwrap();
    let pending_before = w.db.with(|c| repo::documents_to_index(c, None, false)).unwrap();
    assert!(pending_before.iter().any(|d| d.id == "sec"));
    let report = index_pending(&w.index_deps(), None, false, None, &mut |_| {}).unwrap();
    assert_eq!(report.unchanged, 1);
    assert!(w.db.with(|c| repo::documents_to_index(c, None, false)).unwrap().is_empty());
    assert_eq!(index_status(&w.index_deps()).unwrap().documents_pending, 0);
    // Explicit ids are indexed even when nothing changed (force path through ids).
    let only = w.db.with(|c| repo::documents_to_index(c, Some(&["con".to_string()]), false)).unwrap();
    assert_eq!(only.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), vec!["con"]);
    // A different tokenizer / chunk size recorded in kv forces a full re-chunk.
    w.db.with(|c| repo::kv_set(c, super::index::KV_INDEX_SIGNATURE, "v1|other@x|target=99")).unwrap();
    let report = index_pending(&w.index_deps(), Some(&["con".to_string()]), false, None, &mut |_| {}).unwrap();
    assert_eq!(report.indexed.len(), 6, "{report:?}");
    let sig = w.db.with(|c| repo::kv_get(c, super::index::KV_INDEX_SIGNATURE)).unwrap().unwrap();
    assert!(sig.contains("target="), "{sig}");
}

#[test]
fn importing_into_a_deleted_collection_is_a_typed_error_and_feedback_is_validated() {
    let w = World::new();
    assert!(!w.db.with(|c| repo::collection_exists(c, "gone")).unwrap());
    let err = w.db.with(|c| repo::add_docs_to_collection(c, "gone", &["sec".into()])).unwrap_err();
    assert!(err.to_string().contains("collection_not_found"), "{err}");
    let err =
        w.db.with(|c| {
            repo::set_version(
                c,
                &VersionInput { document_id: "nope".into(), group_key: "g".into(), ..VersionInput::default() },
            )
        })
        .unwrap_err();
    assert!(err.to_string().contains("document_not_found"), "{err}");
    let col =
        w.db.with(|c| repo::save_collection(c, &CollectionInput { name: "A".into(), ..CollectionInput::default() }))
            .unwrap();
    let err =
        w.db.with(|c| repo::save_collection(c, &CollectionInput { name: "a".into(), ..CollectionInput::default() }))
            .unwrap_err();
    assert!(err.to_string().contains("collection_name_taken"), "{err}");
    let err =
        w.db.with(|c| {
            repo::save_collection(
                c,
                &CollectionInput { id: Some("missing".into()), name: "B".into(), ..CollectionInput::default() },
            )
        })
        .unwrap_err();
    assert!(err.to_string().contains("collection_not_found"), "{err}");
    let g = w
        .db
        .with(|c| repo::save_glossary(c, &GlossaryInput { id: None, term: "VAT".into(), ..GlossaryInput::default() }))
        .unwrap();
    let err =
        w.db.with(|c| {
            repo::save_glossary(c, &GlossaryInput { id: Some(g.id), term: "출장비".into(), ..GlossaryInput::default() })
        })
        .unwrap_err();
    assert!(err.to_string().contains("glossary_term_taken"), "{err}");
    assert!(w.db.with(|c| repo::collection_exists(c, &col.id)).unwrap());
}

#[test]
fn embedding_prefixes_are_applied_to_documents_and_queries() {
    // Records every input the engine would see; vectors come from the hash embedder.
    struct Recording {
        inner: HashEmbedder,
        prefixes: embed::EmbedPrefixes,
        inputs: std::sync::Mutex<Vec<String>>,
    }
    impl Embedder for Recording {
        fn model_id(&self) -> &str {
            "embed-gemma2-test"
        }
        fn embed(&self, inputs: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
            self.inputs.lock().unwrap().extend(inputs.iter().cloned());
            self.inner.embed(inputs)
        }
        fn prefixes(&self) -> embed::EmbedPrefixes {
            self.prefixes.clone()
        }
    }
    let prefixes = embed::EmbedPrefixes::from_defaults(&serde_json::json!({
        "pooling": "mean",
        "query_prefix": "task: search result | query: ",
        "document_prefix": "title: none | text: "
    }));
    assert_eq!(prefixes.query, "task: search result | query: ");
    assert_eq!(
        embed::EmbedPrefixes::from_defaults(&serde_json::json!({ "pooling": "cls" })),
        embed::EmbedPrefixes::default()
    );
    let w = World::new();
    let rec = Recording { inner: HashEmbedder::new(16), prefixes: prefixes.clone(), inputs: Default::default() };
    let report = embed::embed_pending(&w.db, &rec, 8, None, &mut |_, _| {}).unwrap();
    assert!(report.embedded > 0 && report.failed == 0, "{report:?}");
    {
        let seen = rec.inputs.lock().unwrap();
        assert_eq!(seen.len() as u64, report.embedded);
        assert!(seen.iter().all(|s| s.starts_with("title: none | text: ")), "{:?}", seen.first());
        // The cap applies to the text, the prefix comes on top of it.
        assert!(seen.iter().all(|s| s.chars().count() <= embed::MAX_EMBED_CHARS + prefixes.document.chars().count()));
        // The stored chunk text itself carries no prefix.
        let chunk = w.db.with(|c| repo::list_chunks(c, "con")).unwrap()[0].clone();
        assert!(!chunk.text.starts_with("title:"));
    }
    rec.inputs.lock().unwrap().clear();
    let long_query: String = "지체상금 ".repeat(600);
    let q = embed::embed_query(&rec, &long_query).unwrap();
    assert_eq!(q.len(), 16);
    let seen = rec.inputs.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].starts_with("task: search result | query: 지체상금"));
    assert_eq!(seen[0].chars().count(), embed::MAX_EMBED_CHARS + prefixes.query.chars().count());
    // A prefix-free embedder (bge-style) is unchanged.
    let plain = HashEmbedder::new(8);
    assert_eq!(plain.prefixes(), embed::EmbedPrefixes::default());
}

#[test]
fn embedding_aborts_after_consecutive_failures_and_skips_replaced_chunks() {
    struct FlakySync {
        fail_after: usize,
        calls: std::sync::Mutex<usize>,
    }
    impl Embedder for FlakySync {
        fn model_id(&self) -> &str {
            "flaky"
        }
        fn embed(&self, inputs: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
            let mut c = self.calls.lock().unwrap();
            *c += 1;
            if *c > self.fail_after {
                return Err(crate::error::CoreError::Engine("engine gone".into()));
            }
            Ok(inputs.iter().map(|_| vec![1.0, 0.0, 0.0, 0.0]).collect())
        }
    }
    let w = World::new();
    let flaky = FlakySync { fail_after: 1, calls: std::sync::Mutex::new(0) };
    let err = embed::embed_pending(&w.db, &flaky, 2, None, &mut |_, _| {}).unwrap_err();
    assert!(err.to_string().contains("engine gone"), "{err}");
    assert_eq!(*flaky.calls.lock().unwrap(), 1 + 3, "one success, then three consecutive failures abort");
    // The store step only marks the chunk whose text was embedded: a stale hash matches nothing.
    let chunk = w.db.with(|c| repo::list_chunks(c, "sec")).unwrap()[0].clone();
    let n =
        w.db.with(|c| {
            c.execute(
                "UPDATE chunks SET embedded_at = ?2 WHERE id = ?1 AND text_hash = ?3 AND embedded_at IS NULL",
                rusqlite::params![chunk.id, "now", "stale-hash"],
            )
        })
        .unwrap();
    assert_eq!(n, 0);
    let ok = HashEmbedder::new(8);
    let report = embed::embed_pending(&w.db, &ok, 64, None, &mut |_, _| {}).unwrap();
    assert_eq!(report.failed, 0, "{report:?}");
    assert!(report.embedded > 0);
}

/// KNO-1: evidence kept from the previous answer stays inside the scope of the follow-up. A question asked
/// about an attached document must not cite (or be answered from) a document outside that scope.
#[test]
fn kept_evidence_outside_the_follow_up_scope_is_dropped() {
    let w = World::new();
    let chat = ScriptedChat { reply: "답변입니다[1].".into(), model: "m".into() };
    let deps =
        AnswerDeps { search: w.search_deps(None), chat: &chat, model: None, cancelled: None, context_size: None };
    // First turn: the whole library → the 2024 regulation is cited.
    let first = answer(&deps, &AnswerRequest::question("숙박비 한도는 얼마인가요?"), &mut |_| true).unwrap();
    let kept: Vec<i64> = first.sources.iter().filter(|s| s.document_id == "reg2").map(|s| s.chunk_id).collect();
    assert!(!kept.is_empty(), "{:?}", first.sources);
    // Follow-up about the attached contract only: the regulation is out of scope now.
    let mut follow = AnswerRequest::question("이 계약서의 지체상금 조항은?");
    follow.scope = SearchScope::documents(&["con".to_string()]);
    follow.keep_chunks = kept.clone();
    let a = answer(&deps, &follow, &mut |_| true).unwrap();
    assert!(
        !a.sources.is_empty() && a.sources.iter().all(|s| s.document_id == "con"),
        "{:?}",
        a.sources.iter().map(|s| &s.document_id).collect::<Vec<_>>()
    );
    // Nothing in the new scope matches → "no evidence" instead of the previous sources.
    let silent = ScriptedChat { reply: "SHOULD NOT BE CALLED".into(), model: "m".into() };
    let silent_deps =
        AnswerDeps { search: w.search_deps(None), chat: &silent, model: None, cancelled: None, context_size: None };
    let mut none = AnswerRequest::question("플루토늄 원자로 냉각수 xyzzy");
    none.scope = SearchScope::documents(&["con".to_string()]);
    none.keep_chunks = kept.clone();
    let a = answer(&silent_deps, &none, &mut |_| true).unwrap();
    assert!(a.no_evidence && a.sources.is_empty(), "{:?}", a.sources);
    assert_eq!(a.text, super::answer::no_evidence_text(true));
    // The same scope as before keeps the evidence (the follow-up still leads with it).
    let mut same = AnswerRequest::question("그럼 일비는?");
    same.keep_chunks = kept.clone();
    let a = answer(&deps, &same, &mut |_| true).unwrap();
    assert_eq!(a.sources[0].chunk_id, kept[0]);
    // A superseded edition kept from an explicit-document turn is dropped under the default scope …
    let old = w.db.with(|c| repo::list_chunks(c, "reg1")).unwrap()[0].id;
    let mut latest = AnswerRequest::question("그럼 정산 기한은?");
    latest.keep_chunks = vec![old];
    let a = answer(&deps, &latest, &mut |_| true).unwrap();
    assert!(a.sources.iter().all(|s| s.document_id != "reg1"), "{:?}", a.sources);
    // … but an explanation asked from that very document still draws on it.
    let mut explain = AnswerRequest::question("");
    explain.kind = AnswerKind::Explain;
    explain.selection = Some("숙박비는 1박당 120,000원을 한도로 한다".into());
    explain.document_id = Some("reg1".into());
    explain.keep_chunks = vec![old];
    let a = answer(&deps, &explain, &mut |_| true).unwrap();
    assert!(a.sources.iter().any(|s| s.chunk_id == old), "{:?}", a.sources);
}

/// KNO-7: once the latest edition is cleared, demoted or deleted, the editions it superseded are searchable
/// again — they used to stay flagged "superseded" and out of the default search for good.
#[test]
fn editions_are_searchable_again_once_their_latest_edition_is_gone() {
    let w = World::new();
    let found = |w: &World, doc: &str| w.hits("숙박비 한도").iter().any(|h| h.document_id == doc);
    let version = |w: &World, doc: &str, date: &str, latest: bool| {
        w.db.with(|c| {
            repo::set_version(
                c,
                &VersionInput {
                    document_id: doc.into(),
                    group_key: "출장비 규정".into(),
                    version_label: None,
                    effective_date: Some(date.into()),
                    is_latest: latest,
                    note: None,
                },
            )
        })
        .unwrap()
    };
    let is_latest = |w: &World, doc: &str| w.db.with(|c| repo::get_version(c, doc)).unwrap().is_some_and(|v| v.is_latest);
    // A third, older edition (2022) joins the 2023 / 2024 editions of the fixture.
    add_doc(&w.db, "reg0", "출장비규정_2022.pdf", DocumentRole::Knowledge, &[REGULATION_V1]);
    index_pending(&w.index_deps(), None, false, None, &mut |_| {}).unwrap();
    version(&w, "reg0", "2022-01-01", false);
    assert!(found(&w, "reg2") && !found(&w, "reg1") && !found(&w, "reg0"), "the 2024 edition hides the older ones");

    // 해제: clearing the latest edition promotes the newest remaining one; the oldest stays superseded.
    assert!(w.db.with(|c| repo::clear_version(c, "reg2")).unwrap());
    assert!(is_latest(&w, "reg1") && !is_latest(&w, "reg0"));
    assert!(found(&w, "reg1") && !found(&w, "reg0"));
    assert!(found(&w, "reg2"), "the cleared document is an ordinary document again");
    assert!(!w.db.with(|c| repo::clear_version(c, "reg2")).unwrap(), "nothing left to clear");
    // Clearing a superseded edition changes nothing for the others.
    assert!(w.db.with(|c| repo::clear_version(c, "reg0")).unwrap());
    assert!(is_latest(&w, "reg1"));
    version(&w, "reg0", "2022-01-01", false);

    // The latest edition's document is deleted (its version row goes with it): the group has no latest any
    // more, so what is left is searchable.
    w.db.with(|c| docs::delete_document(c, "reg1")).unwrap();
    assert!(!is_latest(&w, "reg0"), "no write happened on that path");
    assert!(found(&w, "reg0"), "a group without a latest edition hides nothing");

    // Demoting the latest edition without naming another one has the same effect.
    version(&w, "reg2", "2024-01-01", true);
    assert!(found(&w, "reg2") && !found(&w, "reg0"));
    version(&w, "reg2", "2024-01-01", false);
    assert!(found(&w, "reg2") && found(&w, "reg0"));
    // `latest_only = false` still shows every edition, and the listing leads with the newest.
    let groups = w.db.with(repo::list_version_groups).unwrap();
    assert_eq!(groups[0].documents.iter().map(|d| d.document_id.as_str()).collect::<Vec<_>>(), vec!["reg2", "reg0"]);
}

/// PRJ-16: looking at a project's reference group does not create it. The screen gets an unsaved group under
/// the id the group will have; the row is written when the first document is added — opening a project used
/// to leave an empty group behind (one more in the settings' list for every project ever opened).
#[test]
fn a_project_reference_group_is_written_on_first_use_not_when_it_is_shown() {
    use crate::projects::{repo as projects, ProjectInput};
    let w = World::new();
    let project =
        w.db.with(|c| projects::save_project(c, &ProjectInput { name: "한빛 납품".into(), ..Default::default() }))
            .unwrap();
    // What the project screen asks for when it opens.
    let shown = w.db.with_read(|c| repo::peek_project_reference_group(c, &project.id)).unwrap();
    assert_eq!(
        (shown.kind, shown.project_id.as_deref(), shown.name.as_str(), shown.active, shown.doc_count),
        (CollectionKind::Project, Some(project.id.as_str()), "한빛 납품", true, 0)
    );
    assert_eq!(shown.id, repo::project_group_id(&project.id));
    assert!(w.db.with(repo::list_collections).unwrap().is_empty(), "nothing was written");
    assert_eq!(w.db.with(repo::index_status).unwrap().collections, 0);
    assert!(w.db.with(|c| repo::reference_group_of_project(c, &project.id)).unwrap().is_none());
    // The id works wherever a collection id goes: it is a valid target and reads as an empty group.
    assert!(w.db.with(|c| repo::collection_exists(c, &shown.id)).unwrap());
    assert!(w.db.with(|c| repo::collection_documents(c, &shown.id)).unwrap().is_empty());
    assert_eq!(w.db.with(|c| repo::remove_docs_from_collection(c, &shown.id, &["sec".into()])).unwrap(), 0);
    let scope = SearchScope { collections: vec![shown.id.clone()], ..SearchScope::default() };
    assert!(w.db.with(|c| repo::scope_doc_ids(c, &scope)).unwrap().unwrap().is_empty());
    // The first document writes the group — under the same id, so what the screen holds stays valid.
    assert_eq!(w.db.with(|c| repo::add_docs_to_collection(c, &shown.id, &["sec".into()])).unwrap(), 1);
    let stored = w.db.with(|c| repo::reference_group_of_project(c, &project.id)).unwrap().unwrap();
    assert_eq!((stored.id.as_str(), stored.kind, stored.name.as_str(), stored.doc_count), (shown.id.as_str(), CollectionKind::Project, "한빛 납품", 1));
    assert_eq!(w.db.with_read(|c| repo::peek_project_reference_group(c, &project.id)).unwrap(), stored);
    assert!(w.db.with(|c| repo::project_reference_doc_ids(c, &project.id)).unwrap().contains("sec"));
    assert_eq!(w.db.with(repo::list_collections).unwrap().len(), 1);
    // The writing variant (planner, "참고 자료로 넣기") and a group made through the form use that id as well.
    let second =
        w.db.with(|c| projects::save_project(c, &ProjectInput { name: "두 번째".into(), ..Default::default() })).unwrap();
    let created = w.db.with(|c| repo::project_reference_group(c, &second.id, &second.name)).unwrap();
    assert_eq!(created.id, repo::project_group_id(&second.id));
    let third =
        w.db.with(|c| projects::save_project(c, &ProjectInput { name: "세 번째".into(), ..Default::default() })).unwrap();
    let via_form =
        w.db.with(|c| {
            repo::save_collection(
                c,
                &CollectionInput { name: "임시".into(), project_id: Some(third.id.clone()), ..CollectionInput::default() },
            )
        })
        .unwrap();
    assert_eq!(via_form.id, repo::project_group_id(&third.id));
    // A group from before 0.8.0 keeps its random id; the derived id is then not a collection.
    let old =
        w.db.with(|c| projects::save_project(c, &ProjectInput { name: "예전 프로젝트".into(), ..Default::default() }))
            .unwrap();
    w.db.with(|c| {
        c.execute(
            "INSERT INTO collections (id, name, active, kind, project_id, created_at, updated_at) \
             VALUES ('legacy-group', '예전 프로젝트', 1, 'project', ?1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [&old.id],
        )
    })
    .unwrap();
    assert_eq!(w.db.with_read(|c| repo::peek_project_reference_group(c, &old.id)).unwrap().id, "legacy-group");
    let derived = repo::project_group_id(&old.id);
    assert!(!w.db.with(|c| repo::collection_exists(c, &derived)).unwrap());
    let err = w.db.with(|c| repo::add_docs_to_collection(c, &derived, &["sec".into()])).unwrap_err();
    assert!(err.to_string().contains("collection_not_found"), "{err}");
    // Unknown projects and made-up ids are still errors.
    assert!(!w.db.with(|c| repo::collection_exists(c, "pg-nope")).unwrap());
    let err = w.db.with(|c| repo::add_docs_to_collection(c, "pg-nope", &["sec".into()])).unwrap_err();
    assert!(err.to_string().contains("collection_not_found"), "{err}");
    let err = w.db.with_read(|c| repo::peek_project_reference_group(c, "nope")).unwrap_err();
    assert!(err.to_string().contains("project_not_found"), "{err}");
    // Deleting the project takes its group along, written or not.
    assert!(w.db.with(|c| projects::delete_project(c, &project.id)).unwrap());
    assert!(w.db.with(|c| repo::get_collection(c, &shown.id)).unwrap().is_none());
    assert!(!w.db.with(|c| repo::collection_exists(c, &shown.id)).unwrap());
}

/// PRJ-4: the role follows the memberships. A regulation that is common reference material stays reference
/// material when one project switches its own use of it to "업무 문서" — the switch used to make it a work
/// document everywhere (and, with a document retention period, a candidate for the next purge).
#[test]
fn a_document_in_a_reference_group_stays_reference_material() {
    use crate::projects::{repo as projects, ProjectInput};
    let w = World::new();
    let role = |id: &str| w.db.with(|c| docs::get_document(c, id)).unwrap().unwrap().role;
    let common =
        w.db.with(|c| repo::save_collection(c, &CollectionInput { name: "사규".into(), ..CollectionInput::default() }))
            .unwrap();
    let project =
        w.db.with(|c| projects::save_project(c, &ProjectInput { name: "한빛 납품".into(), ..Default::default() }))
            .unwrap();
    let group = w.db.with(|c| repo::project_reference_group(c, &project.id, &project.name)).unwrap();
    w.db.with(|c| {
        repo::add_docs_to_collection(c, &common.id, &["sec".into()])?;
        repo::add_docs_to_collection(c, &group.id, &["sec".into()])
    })
    .unwrap();
    assert!(w.db.with(|c| repo::in_reference_group(c, "sec")).unwrap());
    assert!(!w.db.with(|c| repo::in_reference_group(c, "con")).unwrap());
    // The project turns the guideline into one of its work documents (the three calls of the kind switch).
    w.db.with(|c| {
        repo::remove_docs_from_collection(c, &group.id, &["sec".into()])?;
        projects::add_documents(c, &project.id, &["sec".into()])?;
        Ok(())
    })
    .unwrap();
    let now = w.db.with(|c| repo::set_role_by_membership(c, "sec", DocumentRole::Work)).unwrap();
    assert_eq!((now, role("sec")), (DocumentRole::Knowledge, DocumentRole::Knowledge), "still in the common group");
    assert!(w.db.with(|c| repo::in_reference_group(c, "sec")).unwrap(), "… which also keeps it from the purge");
    assert!(w.db.with(|c| projects::member_ids(c, &project.id)).unwrap().contains(&"sec".to_string()));
    // Out of its last group, the same request makes it a work document.
    w.db.with(|c| repo::remove_docs_from_collection(c, &common.id, &["sec".into()])).unwrap();
    assert!(!w.db.with(|c| repo::in_reference_group(c, "sec")).unwrap());
    assert_eq!(w.db.with(|c| repo::set_role_by_membership(c, "sec", DocumentRole::Work)).unwrap(), DocumentRole::Work);
    assert_eq!(role("sec"), DocumentRole::Work);
    // Every other request is carried out as asked.
    assert_eq!(
        w.db.with(|c| repo::set_role_by_membership(c, "con", DocumentRole::Knowledge)).unwrap(),
        DocumentRole::Knowledge
    );
    w.db.with(|c| repo::add_docs_to_collection(c, &common.id, &["con".into()])).unwrap();
    assert_eq!(w.db.with(|c| repo::set_role_by_membership(c, "con", DocumentRole::Form)).unwrap(), DocumentRole::Form);
    assert_eq!(role("con"), DocumentRole::Form);
}

/// KNO-6: chunks indexed while an embedding run is busy are embedded by that run — it used to stop at the
/// number of chunks pending when it started and leave the new documents without vectors.
#[test]
fn an_embedding_run_picks_up_chunks_indexed_meanwhile() {
    /// Indexes one more document while the first batch is "at the engine".
    struct Interleaving<'a> {
        inner: HashEmbedder,
        world: &'a World,
        late: std::sync::Mutex<bool>,
    }
    impl Embedder for Interleaving<'_> {
        fn model_id(&self) -> &str {
            self.inner.model_id()
        }
        fn embed(&self, inputs: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
            let mut late = self.late.lock().unwrap();
            if !*late {
                *late = true;
                let text = format!("{REGULATION_V2}\n제6조(부칙) 임베딩 도중에 색인된 문서이다.");
                add_doc(&self.world.db, "late", "나중규정.pdf", DocumentRole::Knowledge, &[&text]);
                let report = index_pending(&self.world.index_deps(), None, false, None, &mut |_| {}).unwrap();
                assert_eq!(report.indexed, vec!["late".to_string()]);
                assert!(report.chunks >= 3, "the late document must outnumber one batch: {}", report.chunks);
            }
            self.inner.embed(inputs)
        }
    }
    let w = World::new();
    let before = w.db.with(embed::pending_count).unwrap();
    let embedder = Interleaving { inner: HashEmbedder::new(16), world: &w, late: std::sync::Mutex::new(false) };
    let mut shown: Vec<(u64, u64)> = Vec::new();
    let report = embed::embed_pending(&w.db, &embedder, 2, None, &mut |done, total| shown.push((done, total))).unwrap();
    let (total, embedded) = w.db.with(repo::chunk_counts).unwrap();
    assert!(total > before, "the late document added chunks");
    assert_eq!((embedded, report.remaining, report.failed), (total, 0, 0), "{report:?}");
    assert_eq!(report.embedded, total);
    assert!(w.db.with(|c| repo::list_chunks(c, "late")).unwrap().iter().all(|c| c.embedded));
    // The progress total follows the run instead of ending above 100 %.
    assert!(shown.iter().all(|(done, total)| done <= total), "{shown:?}");
    assert_eq!(shown.last(), Some(&(total, total)));
    // The new chunks are searchable by meaning straight away.
    let mut req = SearchRequest::new("임베딩 도중에 색인된 문서");
    req.mode = SearchMode::Semantic;
    let hits = search(&w.search_deps(Some(&embedder.inner)), &req).unwrap().hits;
    assert!(hits.iter().any(|h| h.document_id == "late"), "{:?}", hits.iter().map(|h| &h.document_id).collect::<Vec<_>>());
}

/// KNO-8: a glossary term the analyzer splits into morphemes (공급가액 → 공급 + 가액 under ko-dic) and a
/// synonym of several words ("net amount") expand the query like a single-word term does.
#[test]
fn glossary_expansion_covers_compounds_and_multi_word_synonyms() {
    let w = World::new();
    add_doc(
        &w.db,
        "inv",
        "invoice-summary.pdf",
        DocumentRole::Knowledge,
        &["Invoice summary\n\nThe net amount is KRW 10,909,091 before tax."],
    );
    index_pending(&w.index_deps(), None, false, None, &mut |_| {}).unwrap();
    w.db.with(|c| {
        repo::save_glossary(
            c,
            &GlossaryInput {
                id: None,
                term: "공급가액".into(),
                definition: None,
                synonyms: vec!["net amount".into()],
            },
        )
    })
    .unwrap();
    let docs_of = |resp: &SearchResponse| resp.hits.iter().map(|h| h.document_id.clone()).collect::<Vec<_>>();
    // Korean compound (with a particle under ko-dic) → the English synonym.
    let query = if w.analyzer.kind() == AnalyzerKind::KoDic { "공급가액은 얼마야?" } else { "공급가액 합계" };
    let resp = search(&w.search_deps(None), &SearchRequest::new(query)).unwrap();
    assert_eq!(resp.expanded_terms, vec!["net amount".to_string()], "terms {:?}", resp.query_terms);
    assert!(docs_of(&resp).contains(&"inv".to_string()), "{:?}", docs_of(&resp));
    // Synonym of two words → the Korean term (the contract states the 공급가액).
    let resp = search(&w.search_deps(None), &SearchRequest::new("net amount 합계")).unwrap();
    assert_eq!(resp.expanded_terms, vec!["공급가액".to_string()], "terms {:?}", resp.query_terms);
    assert!(docs_of(&resp).contains(&"con".to_string()), "{:?}", docs_of(&resp));
    // Without the glossary nothing links the two.
    let mut plain = SearchRequest::new("net amount 합계");
    plain.expand_glossary = false;
    assert!(!docs_of(&search(&w.search_deps(None), &plain).unwrap()).contains(&"con".to_string()));
}

/// KNO-8, analyzer-independent: glossary forms are matched as token sequences (here a stand-in for ko-dic
/// that splits compounds and drops suffixes), guarded by the form appearing in the query text.
#[test]
fn glossary_forms_match_as_token_sequences() {
    let entry = |id: i64, term: &str, synonyms: &[&str]| GlossaryEntry {
        id,
        term: term.into(),
        definition: None,
        synonyms: synonyms.iter().map(|s| s.to_string()).collect(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let entries = vec![
        entry(1, "공급가액", &["net amount", "Supply Value"]),
        entry(2, "출장비", &["여비"]),
        entry(3, "가액", &["금액"]),
        entry(4, "단독", &[]),
    ];
    // Splits the compounds a morphological dictionary would split and drops the suffix of 출장비.
    let morphemes = |s: &str| -> Vec<String> {
        let mut out = Vec::new();
        for w in s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
            match w.trim_end_matches(['은', '는']) {
                "공급가액" => out.extend(["공급".to_string(), "가액".to_string()]),
                "출장비" => out.push("출장".to_string()),
                other => out.push(other.to_lowercase()),
            }
        }
        out
    };
    let expand = |query: &str| repo::expand_terms(&entries, query, &morphemes(query), &morphemes);
    let forms = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    // A split compound: each of its terms carries the synonyms; "가액" is a term of its own as well.
    assert_eq!(
        expand("공급가액은 얼마"),
        vec![
            ("공급".to_string(), forms(&["net amount", "Supply Value"])),
            ("가액".to_string(), forms(&["net amount", "Supply Value", "금액"])),
        ]
    );
    // A synonym of several words, whatever its case and spacing in the query.
    assert_eq!(
        expand("what is the Net  Amount"),
        vec![
            ("net".to_string(), forms(&["공급가액", "Supply Value"])),
            ("amount".to_string(), forms(&["공급가액", "Supply Value"])),
        ]
    );
    // The form itself must be in the query: the morpheme 출장 alone is not 출장비.
    assert_eq!(expand("출장 일정"), Vec::<(String, Vec<String>)>::new());
    assert_eq!(expand("출장비 일정"), vec![("출장".to_string(), forms(&["여비"]))]);
    // The parts of a compound in another order are not the compound; a term without synonyms adds nothing.
    assert_eq!(expand("가액 대비 공급 단독"), vec![("가액".to_string(), forms(&["금액"]))]);
    assert!(repo::expand_terms(&entries, "", &[], &morphemes).is_empty());
}

/// Records every request and replies with `reply`, reporting `finish_reason` like the engine does.
struct RecordingChat {
    reply: String,
    finish_reason: Option<String>,
    requests: std::sync::Mutex<Vec<ChatRequest>>,
}

impl RecordingChat {
    fn new(reply: &str, finish_reason: Option<&str>) -> RecordingChat {
        RecordingChat {
            reply: reply.into(),
            finish_reason: finish_reason.map(str::to_string),
            requests: std::sync::Mutex::new(Vec::new()),
        }
    }
    /// `(user prompt, max_tokens)` of the last request.
    fn last(&self) -> (String, u32) {
        let requests = self.requests.lock().unwrap();
        let req = requests.last().expect("a request was sent");
        (req.messages.last().unwrap()["content"].as_str().unwrap().to_string(), req.max_tokens.unwrap_or(0))
    }
}

impl ChatBackend for RecordingChat {
    fn model_id(&self) -> Option<String> {
        Some("recording".into())
    }
    fn chat_stream(&self, req: &ChatRequest, on_delta: &mut dyn FnMut(&str) -> bool) -> Result<ChatResponse> {
        self.requests.lock().unwrap().push(req.clone());
        on_delta(&self.reply);
        Ok(ChatResponse {
            content: self.reply.clone(),
            finish_reason: self.finish_reason.clone(),
            ..ChatResponse::default()
        })
    }
}

/// A pasted text of about 6,000 characters with the instruction before and after it.
fn long_text_request() -> AnswerRequest {
    let body: String =
        (1..=200).map(|i| format!("{i}번째 문장은 금번 과업의 수행 범위와 일정을 설명함. ")).collect();
    let mut req = AnswerRequest::question(&format!("다음 글을 다듬어줘:\n{body}\n위 글을 공문 문체로 정리해 주세요."));
    assert!(req.question.chars().count() > 5_000);
    req.retrieve = false;
    req.save = false;
    req
}

/// KNO-3: a request that brings its own text (다듬기 · 교정 · 번역) is sized from the loaded model's context —
/// not cut at 4,000 characters in and 1,200 tokens out.
#[test]
fn a_text_only_request_is_sized_from_the_model_context() {
    let w = World::new();
    let chat = RecordingChat::new("다듬은 글입니다.", Some("stop"));
    let deps = AnswerDeps {
        search: w.search_deps(None),
        chat: &chat,
        model: None,
        cancelled: None,
        context_size: Some(32_768),
    };
    let a = answer(&deps, &long_text_request(), &mut |_| true).unwrap();
    let (prompt, max_tokens) = chat.last();
    assert!(prompt.contains("1번째 문장") && prompt.contains("200번째 문장"), "the whole text reaches the model");
    assert!(prompt.contains("공문 문체로 정리"), "the closing instruction reaches the model");
    let text_tokens = super::chunker::estimate_tokens(&long_text_request().question);
    assert!(max_tokens >= text_tokens, "the reply may be as long as the text: {max_tokens} < {text_tokens}");
    assert_eq!(a.truncated, None, "nothing was cut");
    assert!(serde_json::to_value(&a).unwrap().get("truncated").is_none(), "the field is absent when nothing was cut");

    // A small context: the text is cut to what fits (head + the closing instruction) and the answer says so.
    let small = AnswerDeps {
        search: w.search_deps(None),
        chat: &chat,
        model: None,
        cancelled: None,
        context_size: Some(4_096),
    };
    let a = answer(&small, &long_text_request(), &mut |_| true).unwrap();
    let (prompt, max_tokens) = chat.last();
    assert_eq!(a.truncated, Some(Truncation::Input));
    assert_eq!(serde_json::to_value(&a).unwrap()["truncated"], "input");
    assert!(prompt.contains("1번째 문장") && !prompt.contains("200번째 문장"), "the tail of the text is dropped");
    assert!(
        prompt.trim_end().ends_with("…\n위 글을 공문 문체로 정리해 주세요."),
        "{:?}",
        prompt.chars().skip(prompt.chars().count().saturating_sub(60)).collect::<String>()
    );
    let prompt_tokens = crate::planner::prompts::approx_tokens(&prompt);
    assert!(prompt_tokens + max_tokens <= 4_096, "prompt {prompt_tokens} + reply {max_tokens} fit the context");
    assert!(max_tokens >= 1_200, "{max_tokens}");

    // The reply stopped at its token limit → marked as cut as well.
    let limited = RecordingChat::new("다듬다 만 글", Some("length"));
    let deps = AnswerDeps { search: w.search_deps(None), chat: &limited, model: None, cancelled: None, context_size: Some(4_096) };
    assert_eq!(answer(&deps, &long_text_request(), &mut |_| true).unwrap().truncated, Some(Truncation::Both));
    let mut short = AnswerRequest::question("다음 문장을 다듬어줘: 금번 과업은 당사가 수행함.");
    short.retrieve = false;
    short.save = false;
    assert_eq!(answer(&deps, &short, &mut |_| true).unwrap().truncated, Some(Truncation::Output));
    assert_eq!(limited.last().1, 1_200, "a short request keeps the standard reply limit");

    // A grounded question keeps the fixed caps (and a reply cut at the limit is marked too).
    let mut grounded = AnswerRequest::question(&format!("출장비 숙박비 한도는? {}", "가".repeat(5_000)));
    grounded.save = false;
    let a = answer(&deps, &grounded, &mut |_| true).unwrap();
    let (prompt, max_tokens) = limited.last();
    assert_eq!(max_tokens, 1_200);
    assert!(prompt.chars().filter(|c| *c == '가').count() < 4_000, "the question is still capped at 4,000 characters");
    assert_eq!(a.truncated, Some(Truncation::Output));
}

/// KNO-3: when the server still refuses a text-only prompt (context unknown, or its tokenizer counts more than
/// the estimate) the text is halved and the answer is marked instead of failing.
#[test]
fn a_text_only_request_the_server_refuses_is_halved_and_marked() {
    let w = World::new();
    let strict = LimitedChat::new(2_000, "다듬은 글입니다.");
    let deps =
        AnswerDeps { search: w.search_deps(None), chat: &strict, model: None, cancelled: None, context_size: None };
    let a = answer(&deps, &long_text_request(), &mut |_| true).unwrap();
    let sizes = strict.sizes();
    assert!(sizes.len() >= 2 && sizes[0] > 2_000 && *sizes.last().unwrap() <= 2_000, "{sizes:?}");
    assert_eq!(a.truncated, Some(Truncation::Input));
    assert_eq!(a.text, "다듬은 글입니다.");
    // Nothing left to halve → the server's refusal is the error.
    let tiny = LimitedChat::new(10, "x");
    let deps =
        AnswerDeps { search: w.search_deps(None), chat: &tiny, model: None, cancelled: None, context_size: None };
    let mut short = AnswerRequest::question("다음 문장을 다듬어줘: 금번 과업은 당사가 수행함.");
    short.retrieve = false;
    let err = answer(&deps, &short, &mut |_| true).unwrap_err();
    assert!(err.to_string().contains("exceeds the available context size"), "{err}");
}
