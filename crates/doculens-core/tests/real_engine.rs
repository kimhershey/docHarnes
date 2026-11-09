//! End-to-end run against a real `llama-server` (design §4.1 P1 acceptance:
//! load / switch / chat on the CPU build). Skipped unless both variables are
//! set:
//!
//! * `DOCULENS_LLAMA_SERVER` — path of the llama-server binary
//! * `DOCULENS_TEST_MODELS_DIR` — a drop-in models directory (one folder per
//!   model with `*.gguf` + optional `mmproj*.gguf`)
//!
//! Run with `cargo test -p doculens-core --test real_engine -- --nocapture`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use doculens_core::engine::client::{data_url, user_message_with_image};
use doculens_core::engine::{devices, ChatRequest, EngineConfig, EngineManager, EngineState, PresetContext};
use doculens_core::engine::{locate, presets};
use doculens_core::hw::{Backend, Tier};
use doculens_core::modelstore::{scan_models_dir, ScanOptions};
use doculens_core::pipeline::prompts::{self, Stage};
use doculens_core::pipeline::vlm::Vlm;
use doculens_core::pipeline::{self, DocTypeInfo};
use doculens_core::registry::Registry;
use doculens_core::settings::{ModelSettings, Profile};

/// 1×1 red PNG.
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49,
    0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0,
    0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

#[test]
fn real_llama_server_load_chat_switch() {
    let (Some(bin), Some(models_dir)) =
        (std::env::var_os("DOCULENS_LLAMA_SERVER"), std::env::var_os("DOCULENS_TEST_MODELS_DIR"))
    else {
        eprintln!("real_engine: skipped (set DOCULENS_LLAMA_SERVER and DOCULENS_TEST_MODELS_DIR)");
        return;
    };
    let bin = PathBuf::from(bin);
    let models_dir = PathBuf::from(models_dir);
    // A fresh checkout makes the fixture files younger than the copy-settle
    // window (they would scan as "copying"); they are complete by construction.
    for entry in std::fs::read_dir(&models_dir).into_iter().flatten().flatten() {
        if let Ok(files) = std::fs::read_dir(entry.path()) {
            for f in files.flatten() {
                if f.path().extension().is_some_and(|e| e == "gguf") {
                    doculens_core::modelstore::mark_settled(&f.path());
                }
            }
        }
    }
    let registry = Registry::builtin();
    let discovered = scan_models_dir(&models_dir, &registry, ScanOptions::default());
    let ready: Vec<_> = discovered.iter().filter(|d| d.health.is_ready()).collect();
    assert!(
        !ready.is_empty(),
        "no loadable model in {}: {:?}",
        models_dir.display(),
        discovered.iter().map(|d| (&d.folder_name, &d.health)).collect::<Vec<_>>()
    );
    for d in &ready {
        eprintln!(
            "model {} ({}) arch={:?} quant={:?} vision={} chat_template={} warnings={:?}",
            d.id, d.display, d.architecture, d.quantization, d.has_vision, d.has_chat_template, d.warnings
        );
    }

    let binary = locate::EngineBinary {
        path: bin.clone(),
        backend: Backend::Cpu,
        version: None,
        build_number: None,
        source: locate::EngineSource::Env,
    }
    .with_probed_version();
    eprintln!("engine {} version {:?} (build {:?})", bin.display(), binary.version, binary.build_number);
    assert!(!binary.is_too_old(), "engine build {:?} is older than the supported minimum", binary.build_number);
    let devs = devices::list_devices(&bin, Duration::from_secs(60)).expect("--list-devices");
    eprintln!("devices: {devs:?}");

    let appdata = tempfile::tempdir().unwrap();
    let settings = ModelSettings::default();
    let first = ready[0].id.clone();
    let ctx = PresetContext {
        backend: Backend::Cpu,
        tier: Tier::CpuOnly,
        profile: Profile::Speed,
        settings: &settings,
        active_model: None,
        debug: false,
    };
    let build = presets::build_presets(&registry, &discovered, &ctx);
    assert!(
        build.skipped.iter().all(|(id, _)| !ready.iter().any(|r| r.id == *id)),
        "ready model skipped: {:?}",
        build.skipped
    );
    let presets_path = appdata.path().join("presets.ini");
    build.file.write(&presets_path).unwrap();
    eprintln!("presets.ini:\n{}", build.file.to_ini());

    let mut cfg = EngineConfig::new(binary, presets_path, appdata.path().to_path_buf());
    cfg.sleep_idle_seconds = 0;
    cfg.health_timeout = Duration::from_secs(120);
    cfg.mmproj_sizes = build.mmproj_sizes.clone();
    let m = EngineManager::new();
    let events = m.subscribe();
    let t0 = Instant::now();
    let st = m.start(cfg).expect("engine start");
    eprintln!("router up in {} ms: pid {:?} port {:?}", t0.elapsed().as_millis(), st.pid, st.port);
    assert_eq!(st.state, EngineState::Running);
    assert!(st.models.iter().any(|m| m.id == first), "router does not list {first}: {:?}", st.models);

    let t1 = Instant::now();
    let loaded = m.load_model(&first, Duration::from_secs(600), true).expect("load model");
    eprintln!("loaded {first} in {} ms: {:?}", t1.elapsed().as_millis(), loaded);
    assert!(loaded.is_loaded());
    for l in m.recent_logs(400).iter().filter(|l| {
        l.text.contains("load_backend")
            || l.text.contains("system_info")
            || l.text.contains("flash_attn")
            || l.text.contains("n_ctx")
    }) {
        eprintln!("  engine: {}", l.text);
    }
    // On failure, print what the engine said (backend variant, kernel errors) before panicking.
    let dump = |m: &EngineManager, what: &str, e: &dyn std::fmt::Display| -> ! {
        let tail: Vec<String> = m.recent_logs(80).iter().map(|l| format!("  {:?} {}", l.stream, l.text)).collect();
        panic!("{what}: {e}\nengine log tail:\n{}", tail.join("\n"));
    };

    let r = m.chat(&ChatRequest::text(&first, "Say hello.").max_tokens(8)).unwrap_or_else(|e| dump(&m, "chat", &e));
    eprintln!("chat → {:?} finish={:?} usage={:?} timings={:?}", r.content, r.finish_reason, r.usage, r.timings);
    assert!(r.usage.is_some_and(|u| u.completion_tokens >= 1));

    let mut deltas = 0;
    let s = m
        .chat_stream(&ChatRequest::text(&first, "Count: one two three").max_tokens(8), &mut |_| {
            deltas += 1;
            true
        })
        .unwrap_or_else(|e| dump(&m, "stream", &e));
    eprintln!("stream → {} deltas, {:?}", deltas, s.content);
    assert!(deltas >= 1);

    let schema = serde_json::json!({ "type": "object", "properties": { "ok": { "type": "boolean" } }, "required": ["ok"], "additionalProperties": false });
    let j = m
        .chat(&ChatRequest::text(&first, "Reply with ok=true.").max_tokens(64).json_schema("probe", schema))
        .unwrap_or_else(|e| dump(&m, "json schema chat", &e));
    eprintln!("json_schema → {:?} (finish {:?})", j.content, j.finish_reason);
    // Random weights may pad with whitespace until the token budget ends; a
    // complete answer must be valid JSON, a cut-off one at least a JSON prefix.
    if j.finish_reason.as_deref() == Some("stop") {
        let parsed: serde_json::Value =
            serde_json::from_str(j.content.trim()).expect("constrained output must be valid JSON");
        assert!(parsed.get("ok").is_some());
    } else {
        assert!(
            j.content.trim_start().starts_with('{'),
            "constrained output does not start a JSON object: {}",
            j.content
        );
    }

    if ready[0].has_vision {
        let req = ChatRequest {
            model: first.clone(),
            messages: vec![user_message_with_image("What is this?", &data_url("image/png", TINY_PNG))],
            max_tokens: Some(8),
            ..Default::default()
        };
        let v = m.chat(&req).unwrap_or_else(|e| dump(&m, "image chat", &e));
        eprintln!("image chat → {:?} usage={:?}", v.content, v.usage);
        assert!(v.usage.is_some_and(|u| u.prompt_tokens > 10), "image did not add prompt tokens");

        // P4 pipeline path: the classifier / extractor requests (page image +
        // grammar-constrained JSON, thinking disabled) go through the real server.
        // Random weights may pad until the token budget, so a cut-off reply only
        // has to be a JSON prefix; a complete one must parse.
        let vlm = Vlm::new(&m, first.clone(), Profile::Speed, None);
        let types = vec![DocTypeInfo {
            slug: "receipt".into(),
            label_ko: "영수증".into(),
            label_en: "Receipt".into(),
            schema_id: None,
            builtin: true,
            use_count: 0,
        }];
        let page = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(96, 128, image::Rgb([250, 250, 250])));
        let img = pipeline::image::prepare_image(&page, 64).expect("page image");
        let messages = vec![
            serde_json::json!({ "role": "system", "content": prompts::SYSTEM_JSON }),
            user_message_with_image(&prompts::classify(&types), &img.url),
        ];
        let schema = pipeline::schema::classify_schema(&["receipt".to_string()]);
        let reply = vlm
            .call(Stage::Classify, &messages, Some(("classify", &schema)), Some(48))
            .unwrap_or_else(|e| dump(&m, "pipeline classify", &e));
        eprintln!("pipeline classify → {:?} truncated={} attempts={}", reply.content, reply.truncated, reply.attempts);
        if !reply.truncated {
            let parsed = pipeline::vlm::parse_json(&reply.content).expect("constrained classify output must be JSON");
            let class = pipeline::classify::parse_class(&parsed, &["receipt".to_string()]);
            assert!(["receipt", "other"].contains(&class.doc_type.as_str()));
        } else {
            assert!(reply.content.trim_start().starts_with('{'), "not a JSON prefix: {}", reply.content);
        }
        // The KIE grammar of a built-in schema is accepted by the server (no 400 on the schema).
        let receipt = pipeline::schema::builtins().into_iter().find(|b| b.key == "receipt").unwrap();
        let kie_schema = pipeline::schema::json_schema(&receipt.def);
        let kie = ChatRequest {
            model: first.clone(),
            messages: vec![user_message_with_image("extract", &img.url)],
            max_tokens: Some(24),
            chat_template_kwargs: Some(prompts::template_kwargs()),
            ..Default::default()
        }
        .json_schema("kie_receipt", kie_schema);
        let k = m.chat(&kie).unwrap_or_else(|e| dump(&m, "pipeline kie grammar", &e));
        eprintln!("pipeline kie → {:?} finish={:?}", k.content, k.finish_reason);
        assert!(k.content.trim_start().starts_with('{'), "KIE grammar did not constrain the output: {}", k.content);
        assert!(k.reasoning.is_none(), "thinking leaked despite enable_thinking=false");

        // P5 planner path: the PlanSchema grammar (anyOf over every op object) is accepted by
        // the server, and the planner ends with a model plan or a clarification — never with a
        // transport / grammar error. Random weights produce junk inside the grammar, so only
        // the constrained shape and the error handling are checked here.
        let pdeps = doculens_core::planner::llm::PlannerDeps {
            chat: &m,
            model: first.clone(),
            profile: Profile::Speed,
            ctx_size: m.status().model(&first).and_then(|x| x.ctx),
            cancel: None,
        };
        let pctx = doculens_core::planner::PlanContext {
            doc_types: vec![doculens_core::planner::DocTypeSummary {
                slug: "receipt".into(),
                label_ko: "영수증".into(),
                label_en: "Receipt".into(),
            }],
            ..Default::default()
        };
        let t = Instant::now();
        let planned = doculens_core::planner::llm::plan(
            Some(&pdeps),
            "한빛과 거래한 계약 중 작년 것만 골라서 보여줘",
            &pctx,
            doculens_core::planner::PlanMode::ModelFirst,
        )
        .expect("planner call");
        eprintln!(
            "planner → source={:?} attempts={} intent={:?} issues={:?} raw={:?} in {} ms",
            planned.source,
            planned.attempts,
            planned.plan.intent,
            planned.issues.iter().map(|i| i.code.as_str()).collect::<Vec<_>>(),
            planned.raw.as_deref().map(|r| r.chars().take(160).collect::<String>()),
            t.elapsed().as_millis()
        );
        // Random weights sometimes emit tool-call fragments the server's output parser rejects
        // (HTTP 500 "does not match the expected … format", the P4 smoke artefact); that is a model
        // artefact, not a grammar / transport problem, so only other engine errors fail the test.
        // Likewise a reply that runs into the reply budget ("cut off (1024 tokens …)") is random weights
        // rambling inside the grammar; a reply clamped by a full slot is retried at a smaller tier by the
        // planner itself, so only a cut-off at the budget can reach here.
        let output_parser_500 =
            planned.issues.iter().any(|i| i.code == "model_error" && i.message.contains("does not match the expected"));
        let rambling = planned.issues.iter().any(|i| i.code == "model_error" && i.message.contains("was cut off"));
        assert!(
            output_parser_500 || rambling || !planned.issues.iter().any(|i| i.code == "model_error"),
            "planner grammar / transport error: {:?}",
            planned.issues
        );
        if !output_parser_500 && !rambling {
            assert!(
                planned.raw.as_deref().is_some_and(|r| r.trim_start().starts_with('{')),
                "plan output not constrained: {:?}",
                planned.raw
            );
        }
    }

    // Every other grammar the app sends through the real server: the form / NL-schema metaschema
    // (양식 등록·NL 스키마, user report 2026-11-09 "양식 등록이 되지 않음"), the document-draft schema
    // (§3.13) and the project-rule schema (§3.15). Random weights produce junk inside the grammar, so
    // only acceptance (no 400 on the schema) and the constrained JSON shape are checked.
    if ready[0].has_vision {
        let vlm = Vlm::new(&m, first.clone(), Profile::Speed, None);
        let page = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(96, 128, image::Rgb([250, 250, 250])));
        let img = pipeline::image::prepare_image(&page, 64).expect("page image");
        let t = Instant::now();
        match pipeline::forms::derive_with_vlm(
            &vlm,
            Some(&img.url),
            Some("휴가신청서\n성명:____ 부서:____ 기간: 년 월 일 ~ 년 월 일"),
            Some("휴가신청서"),
        ) {
            Ok(d) => eprintln!(
                "form_fields → {} fields, doc_type {} in {} ms",
                d.definition.fields.len(),
                d.doc_type,
                t.elapsed().as_millis()
            ),
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("does not match the expected")
                        || msg.contains("was cut off")
                        || msg.contains("not valid JSON")
                        || msg.contains("truncated"),
                    "form_fields grammar / transport error: {msg}"
                );
                eprintln!("form_fields → model artefact (random weights): {msg}");
            }
        }
        match pipeline::forms::nl_schema(&vlm, "발주서. 발주번호, 거래처, 납기일자, 합계금액, 품목", None)
        {
            Ok(d) => eprintln!("nl_schema → {} fields, doc_type {}", d.definition.fields.len(), d.doc_type),
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("does not match the expected")
                        || msg.contains("was cut off")
                        || msg.contains("not valid JSON")
                        || msg.contains("truncated"),
                    "nl_schema grammar / transport error: {msg}"
                );
                eprintln!("nl_schema → model artefact (random weights): {msg}");
            }
        }
        let draft_schema =
            doculens_core::generate::schema::json_schema(&doculens_core::generate::templates::TemplateSpec::builtin(
                doculens_core::generate::TemplateKind::Quotation,
            ));
        let g = m
            .chat(
                &ChatRequest::text(&first, "견적서 초안")
                    .max_tokens(24)
                    .json_schema(doculens_core::generate::schema::SCHEMA_NAME, draft_schema),
            )
            .unwrap_or_else(|e| dump(&m, "generate draft grammar", &e));
        assert!(g.content.trim_start().starts_with('{'), "draft grammar did not constrain the output: {}", g.content);
        let rules_ctx = doculens_core::projects::rules::RuleContext {
            types: vec![doculens_core::projects::rules::TypeInfo {
                slug: "tax_invoice".into(),
                label_ko: "세금계산서".into(),
                label_en: "Tax invoice".into(),
                fields: doculens_core::projects::rules::fallback_fields("tax_invoice"),
                in_scope: true,
                registered: false,
            }],
        };
        let rule_schema = doculens_core::projects::rules::rule_schema(&rules_ctx);
        let r = m
            .chat(
                &ChatRequest::text(&first, "합계금액은 2,000,000 이상")
                    .max_tokens(24)
                    .json_schema("project_rule", rule_schema),
            )
            .unwrap_or_else(|e| dump(&m, "project rule grammar", &e));
        assert!(r.content.trim_start().starts_with('{'), "rule grammar did not constrain the output: {}", r.content);
        eprintln!("draft / rule grammars accepted");
    }

    // Hot swap when a second model exists.
    if let Some(second) = ready.get(1) {
        let t2 = Instant::now();
        let l2 = m.load_model(&second.id, Duration::from_secs(600), false).expect("load second");
        eprintln!("loaded {} in {} ms: {}", second.id, t2.elapsed().as_millis(), l2.status);
        let r2 = m.chat(&ChatRequest::text(&second.id, "hi").max_tokens(4)).expect("chat second");
        eprintln!("second chat → {:?}", r2.content);
    }

    m.unload_model(&first).expect("unload");
    assert_eq!(m.status().model(&first).map(|m| m.status.as_str()), Some("unloaded"));
    let mut n_events = 0;
    while events.try_recv().is_ok() {
        n_events += 1;
    }
    eprintln!("{n_events} events observed");
    assert!(n_events > 0);

    let st = m.restart().expect("restart");
    assert_eq!(st.state, EngineState::Running);
    m.stop().expect("stop");
    assert_eq!(m.status().state, EngineState::Stopped);
    eprintln!(
        "engine log tail:\n{}",
        m.recent_logs(15).iter().map(|l| format!("  {:?} {}", l.stream, l.text)).collect::<Vec<_>>().join("\n")
    );
}
