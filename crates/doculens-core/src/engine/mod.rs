//! llama-server sidecar (design §3.3): binary discovery, preset generation,
//! router API client, process control (job object / process group) and the
//! lifecycle manager with crash recovery. Pure Rust, no UI dependencies; the
//! desktop shell forwards [`manager::EngineEvent`]s to the webview.

pub mod client;
pub mod devices;
pub mod fake_router;
pub mod locate;
pub mod manager;
pub mod presets;
pub mod process;

pub use client::{ChatRequest, ChatResponse, ChatUsage, RouterClient, RouterEvent, RouterModel};
pub use devices::EngineDevice;
pub use locate::{EngineBinary, EngineSource, InstalledPack};
pub use manager::{EngineConfig, EngineEvent, EngineManager, EngineState, EngineStatus, ModelInstanceStatus};
pub use presets::{PresetBuild, PresetContext, PresetFile, PresetModel};
pub use process::{LogLine, LogStream};
