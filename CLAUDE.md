# DocuLens — project instructions for Claude Code sessions

## What this is
Offline, Windows-only desktop client (Tauri 2 + React/TypeScript UI, Rust core)
that processes Korean/English business documents with a local VLM
(Qwen3.5-4B via llama-server). Design: `docs/design/doculens-design-v1.1.md`
(Korean, authoritative). Status and next steps: `docs/HANDOFF.md`.

## Hard rules
- **Licenses**: only dependencies without a source-disclosure obligation
  (`docs/LICENSE-POLICY.md`). No GPL/LGPL/AGPL/SSPL/BUSL/MS-RL/community
  licenses. MPL-2.0 only unmodified. Run `npm run licenses:check` and
  `cargo deny check licenses` before adding dependencies.
- **Korean first**: UI strings default to Korean (`src/i18n/ko.json`) with an
  English twin (`en.json`); the parity test fails on missing keys.
- **No telemetry, no network at runtime** except explicit user-initiated model
  downloads.
- Rust core (`crates/doculens-core`) must stay free of Tauri/UI dependencies.
- Keep `models/registry.json` the single source of truth for models.

## Working method (user decision 2026-11-03)
1. One phase (P0…P5) per session; end each phase with a demo checkpoint
   (tests green, `docs/HANDOFF.md` updated, commit + push).
2. Trim build logs (`| tail`, `grep -v Compiling`); compile-check per module.
3. Spawn review subagents only at the end of a phase.
4. v1.0 scope = P0–P4 + core planner ops; document generation and project
   consistency checks are v1.1.

## Commands
- Rust: `cargo test -p doculens-core`, `cargo check -p doculens`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all`.
- Web: `npm run typecheck`, `npm test` (vitest), `npm run build`,
  `PW_CHROMIUM_PATH=/opt/pw-browsers/chromium npm run e2e` (browser mode, mock backend).
- Desktop (Windows / Linux with webkit2gtk): `npm run tauri dev`, `npm run tauri build`.
- Linux build deps: `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev`.

## Layout
- `crates/doculens-core/` — settings, workspace, hw/tier, memory, registry, retention, prerouter.
- `src-tauri/` — Tauri shell (`src/commands/`, `src/state.rs`), `tauri.conf.json`, capabilities.
- `src/` — React app: `app/` (layout, providers, hooks), `features/<screen>/`, `components/ui/`, `lib/` (ipc, types, theme, shortcuts, mock backend), `stores/`, `i18n/`, `styles/`.
- `models/registry.json`, `docs/`, `scripts/`, `e2e/`, `.github/workflows/`.
