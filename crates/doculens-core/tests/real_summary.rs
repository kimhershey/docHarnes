//! Diagnostic against a real `llama-server`: what a summary of real text looks like under given
//! knowledge settings — how many tokens went in and came out, how long it took, and the text.
//! Written for the user report of 2026-11-17 ("요약이 자꾸 실패", "토큰 많이 설정했는데 출력이 너무
//! 짧음"). Ignored: it needs a real model and takes minutes on a CPU.
//!
//! ```text
//! DOCULENS_LLAMA_SERVER=<llama-server> DOCULENS_TEST_MODELS_DIR=<models dir> \
//! DOCULENS_SUMMARY_SRC=<folder of .txt files, one document each (form feed = page break)> \
//! [DOCULENS_SUMMARY_BUDGETS=6000,24000] [DOCULENS_SUMMARY_CHUNK=1200] [DOCULENS_SUMMARY_MODE=key_points|one_page] \
//! [DOCULENS_SUMMARY_QUESTION=<a question asked of the documents under each budget>] [DOCULENS_KODIC=<ko-dic dir>] \
//! [DOCULENS_SUMMARY_TOPIC=<subject>[|<subject>…]] [DOCULENS_SUMMARY_TOGETHER=1] \
//! cargo test -p doculens-core --test real_summary -- --ignored --nocapture
//! ```
//! (absolute paths: the test binary runs from the crate directory)
//!
//! With `DOCULENS_SUMMARY_TOPIC` (1.0.2, user report of 2026-11-19: "AA에 대해서 요약해줘 … 요약만 하고 AA에
//! 대한 내용을 수집하지 않음") the documents are not summarised one by one: for each subject the passages
//! about it are looked up in all of them and summarised (`summarize_topic`), and the sources are listed.
//!
//! With `DOCULENS_SUMMARY_TOGETHER=1` (1.0.3, user decision of 2026-11-22: "파일별 요약은 없애고 모든 파일의
//! 내용을 하나의 결과로 요약") the documents are summarised as the assistant's `summarize` step does it: all of
//! them in one summary (`summarize_documents`) — the model calls it took and the documents it cites are
//! listed. Without it each document is summarised on its own, as the viewer does for the open document.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use doculens_core::db::Db;
use doculens_core::documents::repo as docs;
use doculens_core::documents::{DocumentRole, NewDocument, NewPage, SourceKind};
use doculens_core::engine::{devices, locate, presets, EngineConfig, EngineManager, PresetContext};
use doculens_core::hw::{Backend, Tier};
use doculens_core::knowledge::answer::{answer, summarize, summarize_documents, summarize_topic, AnswerDeps};
use doculens_core::knowledge::chunker::ChunkOptions;
use doculens_core::knowledge::index::{index_document, IndexDeps};
use doculens_core::knowledge::search::SearchDeps;
use doculens_core::knowledge::tokenize::Analyzer;
use doculens_core::knowledge::{AnswerKind, AnswerRequest, SummaryMode};
use doculens_core::modelstore::{scan_models_dir, ScanOptions};
use doculens_core::registry::Registry;
use doculens_core::settings::{ModelSettings, Profile};

#[test]
#[ignore = "diagnostic with a real model: set DOCULENS_LLAMA_SERVER, DOCULENS_TEST_MODELS_DIR and DOCULENS_SUMMARY_SRC"]
fn summaries_of_real_text_under_given_settings() {
    let (Some(bin), Some(models_dir), Some(src)) = (
        std::env::var_os("DOCULENS_LLAMA_SERVER"),
        std::env::var_os("DOCULENS_TEST_MODELS_DIR"),
        std::env::var_os("DOCULENS_SUMMARY_SRC"),
    ) else {
        eprintln!("real_summary: set DOCULENS_LLAMA_SERVER, DOCULENS_TEST_MODELS_DIR and DOCULENS_SUMMARY_SRC");
        return;
    };
    let (bin, models_dir, src) = (PathBuf::from(bin), PathBuf::from(models_dir), PathBuf::from(src));
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_string());
    let budgets: Vec<u32> =
        var("DOCULENS_SUMMARY_BUDGETS", "6000,24000").split(',').filter_map(|b| b.trim().parse().ok()).collect();
    let chunk: u32 = var("DOCULENS_SUMMARY_CHUNK", "500").parse().expect("chunk size");
    let mode = match var("DOCULENS_SUMMARY_MODE", "key_points").as_str() {
        "one_page" => SummaryMode::OnePage,
        "one_line" => SummaryMode::OneLine,
        _ => SummaryMode::KeyPoints,
    };
    let question = std::env::var("DOCULENS_SUMMARY_QUESTION").ok().filter(|q| !q.trim().is_empty());
    let topics: Vec<String> =
        var("DOCULENS_SUMMARY_TOPIC", "").split('|').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
    let together = !matches!(var("DOCULENS_SUMMARY_TOGETHER", "").trim(), "" | "0");

    // -- the documents ------------------------------------------------------------
    let db = Db::open_in_memory().unwrap();
    // ko-dic when `DOCULENS_KODIC` names it (what an installed copy searches with), else the built-in analyzer.
    let analyzer = Analyzer::load_first(&Analyzer::candidates(&[]));
    eprintln!("analyzer: {:?}", analyzer.kind());
    let mut files: Vec<PathBuf> = std::fs::read_dir(&src)
        .expect("DOCULENS_SUMMARY_SRC is a folder")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "txt"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no .txt file in {}", src.display());
    let mut ids = Vec::new();
    for (n, file) in files.iter().enumerate() {
        let id = format!("d{n}");
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(file).unwrap();
        let pages: Vec<NewPage> = text
            .split('\u{c}')
            .filter(|p| !p.trim().is_empty())
            .enumerate()
            .map(|(i, p)| NewPage { page_no: i as u32 + 1, text: Some(p.trim().to_string()), ..Default::default() })
            .collect();
        db.with(|c| {
            docs::insert_document(
                c,
                &NewDocument {
                    id: id.clone(),
                    rel_path: format!("documents/{name}"),
                    original_name: name.clone(),
                    source_kind: SourceKind::Pdf,
                    content_hash: format!("hash-{n}"),
                    size_bytes: text.len() as u64,
                    file_modified_at: None,
                    role: DocumentRole::Work,
                    import_batch_id: None,
                },
            )
        })
        .unwrap();
        db.with_tx(|tx| docs::replace_pages(tx, &id, &pages, None, false, None)).unwrap();
        let doc = db.with(|c| docs::get_document(c, &id)).unwrap().unwrap();
        let outcome = index_document(
            &IndexDeps { db: &db, analyzer: &analyzer, options: ChunkOptions::with_target(chunk) },
            &doc,
            true,
        )
        .unwrap();
        eprintln!(
            "document {name}: {} characters, {} page(s), chunks of ~{chunk} tokens → {outcome:?}",
            text.chars().count(),
            pages.len()
        );
        ids.push((id, name));
    }

    // -- the engine ---------------------------------------------------------------
    for entry in std::fs::read_dir(&models_dir).into_iter().flatten().flatten() {
        if let Ok(list) = std::fs::read_dir(entry.path()) {
            for f in list.flatten() {
                if f.path().extension().is_some_and(|e| e == "gguf") {
                    doculens_core::modelstore::mark_settled(&f.path());
                }
            }
        }
    }
    let registry = Registry::builtin();
    let discovered = scan_models_dir(&models_dir, &registry, ScanOptions::default());
    let model = discovered.iter().find(|d| d.health.is_ready()).expect("a loadable model").id.clone();
    let binary = locate::EngineBinary {
        path: bin.clone(),
        backend: Backend::Cpu,
        version: None,
        build_number: None,
        source: locate::EngineSource::Env,
    }
    .with_probed_version();
    let _ = devices::list_devices(&bin, Duration::from_secs(60));
    let appdata = tempfile::tempdir().unwrap();
    let settings = ModelSettings::default();
    let ctx = PresetContext {
        backend: Backend::Cpu,
        tier: Tier::CpuOnly,
        profile: Profile::Balanced,
        settings: &settings,
        active_model: None,
        debug: false,
        // As the app does: the model is loaded with a context that holds the largest budget of the run.
        evidence_tokens: budgets.iter().copied().max().unwrap_or(6_000),
    };
    let build = presets::build_presets(&registry, &discovered, &ctx);
    let presets_path = appdata.path().join("presets.ini");
    build.file.write(&presets_path).unwrap();
    let mut cfg = EngineConfig::new(binary, presets_path, appdata.path().to_path_buf());
    cfg.sleep_idle_seconds = 0;
    cfg.health_timeout = Duration::from_secs(120);
    cfg.mmproj_sizes = build.mmproj_sizes.clone();
    let engine = EngineManager::new();
    engine.start(cfg).expect("engine start");
    let t = Instant::now();
    engine.load_model(&model, Duration::from_secs(900), true).expect("load model");
    let context_size = engine.status().model(&model).and_then(|m| m.ctx);
    eprintln!("model {model} loaded in {} s, context {context_size:?}", t.elapsed().as_secs());

    // -- the runs -----------------------------------------------------------------
    let deps = |db: &'static Db, analyzer: &'static Analyzer| SearchDeps { db, analyzer, embedder: None };
    // (the answer functions borrow the database for as long as the engine: leak both for the test)
    let db: &'static Db = Box::leak(Box::new(db));
    let analyzer: &'static Analyzer = Box::leak(Box::new(analyzer));
    let mut failed = 0;
    for budget in &budgets {
        for topic in &topics {
            let adeps = AnswerDeps {
                search: deps(db, analyzer),
                chat: &engine,
                model: Some(model.clone()),
                cancelled: None,
                context_size,
            };
            let mut req = AnswerRequest::question(topic);
            req.kind = AnswerKind::Summary;
            req.summary_mode = mode;
            req.context_tokens = *budget;
            req.save = false;
            let t = Instant::now();
            match summarize_topic(&adeps, &req, &mut |_| true) {
                Ok(a) => {
                    let mut from: Vec<String> = Vec::new();
                    for s in &a.sources {
                        let name = ids.iter().find(|(id, _)| *id == s.document_id).map(|(_, n)| n.clone());
                        let name = name.unwrap_or_else(|| s.document_id.clone());
                        if !from.contains(&name) {
                            from.push(name);
                        }
                    }
                    eprintln!(
                        "\n== about '{topic}' · budget {budget}: {} s · {} passage(s) of {} document(s) {from:?} · nothing found {:?} · prompt {} tokens · reply {} tokens · {} characters · cut {:?}\n{}\n",
                        t.elapsed().as_secs(),
                        a.sources.len(),
                        from.len(),
                        a.no_evidence,
                        a.prompt_tokens,
                        a.completion_tokens,
                        a.text.chars().count(),
                        a.truncated,
                        a.text
                    )
                }
                Err(e) => {
                    failed += 1;
                    eprintln!(
                        "\n== about '{topic}' · budget {budget}: FAILED after {} s: {e}\n",
                        t.elapsed().as_secs()
                    );
                }
            }
        }
        if together && topics.is_empty() {
            let adeps = AnswerDeps {
                search: deps(db, analyzer),
                chat: &engine,
                model: Some(model.clone()),
                cancelled: None,
                context_size,
            };
            let mut req = AnswerRequest::question("");
            req.kind = AnswerKind::Summary;
            req.summary_mode = mode;
            req.context_tokens = *budget;
            req.save = false;
            let all: Vec<String> = ids.iter().map(|(id, _)| id.clone()).collect();
            let t = Instant::now();
            let mut calls = 0;
            let result = summarize_documents(&adeps, &req, &all, &mut |_| true, &mut |made, expected| {
                calls = made;
                eprintln!("   model call {made} of {expected} · {} s", t.elapsed().as_secs());
            });
            match result {
                Ok((a, without_text)) => {
                    let mut from: Vec<String> = Vec::new();
                    for s in &a.sources {
                        let name = ids.iter().find(|(id, _)| *id == s.document_id).map(|(_, n)| n.clone());
                        let name = name.unwrap_or_else(|| s.document_id.clone());
                        if !from.contains(&name) {
                            from.push(name);
                        }
                    }
                    eprintln!(
                        "\n== one summary of {} document(s) · budget {budget}: {} s · {calls} model call(s) · {} cited passage(s) of {} document(s) {from:?} · without text {without_text:?} · prompt {} tokens · reply {} tokens · {} characters · cut {:?}\n{}\n",
                        ids.len(),
                        t.elapsed().as_secs(),
                        a.sources.len(),
                        from.len(),
                        a.prompt_tokens,
                        a.completion_tokens,
                        a.text.chars().count(),
                        a.truncated,
                        a.text
                    )
                }
                Err(e) => {
                    failed += 1;
                    eprintln!(
                        "\n== one summary of {} document(s) · budget {budget}: FAILED after {} s: {e}\n",
                        ids.len(),
                        t.elapsed().as_secs()
                    );
                }
            }
        }
        for (id, name) in ids.iter().filter(|_| topics.is_empty() && !together) {
            let adeps = AnswerDeps {
                search: deps(db, analyzer),
                chat: &engine,
                model: Some(model.clone()),
                cancelled: None,
                context_size,
            };
            let mut req = AnswerRequest::question("");
            req.kind = AnswerKind::Summary;
            req.document_id = Some(id.clone());
            req.summary_mode = mode;
            req.context_tokens = *budget;
            req.save = false;
            let t = Instant::now();
            match summarize(&adeps, &req, &mut |_| true) {
                Ok(a) => eprintln!(
                    "\n== summary · budget {budget} · {name}: {} s · prompt {} tokens · reply {} tokens · {} characters · cut {:?}\n{}\n",
                    t.elapsed().as_secs(),
                    a.prompt_tokens,
                    a.completion_tokens,
                    a.text.chars().count(),
                    a.truncated,
                    a.text
                ),
                Err(e) => {
                    failed += 1;
                    eprintln!("\n== summary · budget {budget} · {name}: FAILED after {} s: {e}\n", t.elapsed().as_secs());
                }
            }
        }
        if let Some(q) = &question {
            let adeps = AnswerDeps {
                search: deps(db, analyzer),
                chat: &engine,
                model: Some(model.clone()),
                cancelled: None,
                context_size,
            };
            let mut req = AnswerRequest::question(q);
            req.context_tokens = *budget;
            req.top_k = 16;
            req.save = false;
            let t = Instant::now();
            match answer(&adeps, &req, &mut |_| true) {
                Ok(a) => eprintln!(
                    "\n== answer · budget {budget}: {} s · {} source(s) · prompt {} tokens · reply {} tokens · {} characters · cut {:?}\n{}\n",
                    t.elapsed().as_secs(),
                    a.sources.len(),
                    a.prompt_tokens,
                    a.completion_tokens,
                    a.text.chars().count(),
                    a.truncated,
                    a.text
                ),
                Err(e) => {
                    failed += 1;
                    eprintln!("\n== answer · budget {budget}: FAILED after {} s: {e}\n", t.elapsed().as_secs());
                }
            }
        }
    }
    let _ = engine.stop();
    assert_eq!(failed, 0, "{failed} request(s) failed");
}
