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
    let exp = db.with(|x| repo::glossary_expansions(x, &["세금".into(), "없음".into()])).unwrap();
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
