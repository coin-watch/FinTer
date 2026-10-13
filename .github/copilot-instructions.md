# GitHub Copilot Instructions for FinTer

## Project Context
**App:** FinTer
**Stack:** Rust, GPUI, Tokio, CCXT (Deno bridge), SQLite
**Stage:** MVP Development

## Directives
1. Read `AGENTS.md` for the current phase and tasks.
2. Refer to `agent_docs/` for tech stack details and code patterns.
3. Follow existing code conventions in the repository.
4. Prioritize performance - verify 60+ FPS for UI features.
5. Write tests for new functionality.
6. Keep changes incremental and focused.
7. Run `cargo fmt --check && cargo clippy` before commits.

## Commands
- `cargo build` — Build the project
- `cargo run --release` — Run in release mode
- `cargo test` — Run tests
- `cargo fmt --check && cargo clippy` — Lint and format check

## Key Constraints
- Must maintain 60+ FPS during 10k+ ticks/sec
- Local-first: core data stays local
- API keys: use keyring-rs for OS secure enclave
