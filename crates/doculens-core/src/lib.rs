//! DocuLens core library.
//!
//! Pure Rust (no UI/Tauri dependencies). Everything the desktop shell needs to
//! bootstrap: global settings, workspace folders, hardware probing & tiering,
//! VRAM budgeting, the model registry and the deterministic natural-language
//! pre-router. Later phases add engine management, the document pipeline,
//! search and the NL planner on top of this crate.
//!
//! Policy: every dependency must be permissively licensed (no source-disclosure
//! obligation). See `docs/LICENSE-POLICY.md`.

pub mod aggregate;
pub mod db;
pub mod documents;
pub mod engine;
pub mod error;
pub mod export;
pub mod fswatch;
pub mod generate;
pub mod gguf;
pub mod hw;
pub mod jobs;
pub mod jsonorder;
pub mod knowledge;
pub mod memory;
pub mod modelstore;
pub mod organize;
pub mod paths;
pub mod pipeline;
pub mod planner;
pub mod prerouter;
pub mod projects;
pub mod registry;
pub mod retention;
pub mod settings;
pub mod version;
pub mod workspace;

pub use error::{CoreError, Result};
