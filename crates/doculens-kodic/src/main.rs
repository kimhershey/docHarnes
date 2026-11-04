//! `doculens-kodic build <mecab-ko-dic source dir> <output dir>` compiles the
//! mecab-ko-dic CSV sources into lindera's binary dictionary layout;
//! `doculens-kodic check <dir>` loads a built dictionary and prints a sample
//! segmentation. `scripts/fetch-kodic.mjs` drives both.
//!
//! The dictionary metadata (`assets/metadata.json`, `assets/context_id_freq.txt`)
//! is the one shipped by the `lindera-ko-dic` crate (MIT); the dictionary data
//! itself is mecab-ko-dic 2.1.1-20180720 (Apache-2.0) and is never committed.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use lindera_dictionary::builder::DictionaryBuilder;
use lindera_dictionary::dictionary::metadata::Metadata;

const METADATA_JSON: &str = include_str!("../assets/metadata.json");
const CONTEXT_ID_FREQ: &str = include_str!("../assets/context_id_freq.txt");
const DICTIONARY_SOURCE: &str = "mecab-ko-dic-2.1.1-20180720";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("build") if args.len() == 3 => build(Path::new(&args[1]), Path::new(&args[2])),
        Some("check") if args.len() == 2 => check(Path::new(&args[1])),
        _ => {
            eprintln!("usage: doculens-kodic build <mecab-ko-dic source dir> <output dir>\n       doculens-kodic check <dictionary dir>");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn build(source: &Path, out: &Path) -> Result<(), String> {
    for required in ["matrix.def", "char.def", "unk.def"] {
        if !source.join(required).is_file() {
            return Err(format!("{} is not a mecab-ko-dic source directory ({required} missing)", source.display()));
        }
    }
    std::fs::create_dir_all(out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    // The builder reads the context-id frequency table from a file next to the metadata.
    let freq_path: PathBuf = out.join("context_id_freq.txt");
    std::fs::write(&freq_path, CONTEXT_ID_FREQ).map_err(|e| format!("cannot write {}: {e}", freq_path.display()))?;
    let metadata: Metadata = serde_json::from_str(METADATA_JSON).map_err(|e| format!("metadata.json: {e}"))?;
    let started = Instant::now();
    DictionaryBuilder::new(metadata)
        .with_context_id_freq(&freq_path)
        .build_dictionary(source, out)
        .map_err(|e| format!("dictionary build failed: {e}"))?;
    let _ = std::fs::remove_file(&freq_path);
    let version = serde_json::json!({
        "dictionary": DICTIONARY_SOURCE,
        "lindera": lindera::get_version(),
        "built_with": env!("CARGO_PKG_VERSION"),
    });
    std::fs::write(out.join("VERSION.json"), serde_json::to_string_pretty(&version).unwrap_or_default())
        .map_err(|e| format!("cannot write VERSION.json: {e}"))?;
    println!("built {} in {:.1}s -> {}", DICTIONARY_SOURCE, started.elapsed().as_secs_f32(), out.display());
    check(out)
}

fn check(dir: &Path) -> Result<(), String> {
    let started = Instant::now();
    let dictionary =
        lindera::dictionary::load_fs_dictionary(dir).map_err(|e| format!("cannot load {}: {e}", dir.display()))?;
    let segmenter = lindera::segmenter::Segmenter::new(lindera::mode::Mode::Normal, dictionary, None);
    let sample = "사업자등록번호와 공급가액은 세금계산서에 기재합니다.";
    let mut tokens = segmenter.segment(Cow::Borrowed(sample)).map_err(|e| format!("segment: {e}"))?;
    let parts: Vec<String> = tokens
        .iter_mut()
        .map(|t| {
            let surface = t.surface.to_string();
            let pos = t.get_detail(0).unwrap_or("?").to_string();
            format!("{surface}/{pos}")
        })
        .collect();
    if !parts.iter().any(|p| p == "공급/NNG") || !parts.iter().any(|p| p == "번호/NNG") {
        return Err(format!("unexpected segmentation: {}", parts.join(" ")));
    }
    println!("ok ({:.0} ms): {}", started.elapsed().as_secs_f64() * 1000.0, parts.join(" "));
    Ok(())
}
