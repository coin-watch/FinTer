# FinTer

FinTer is a native desktop market-monitoring and charting application built with Rust and GPUI. It provides live cryptocurrency market data, interactive candlestick charts, technical indicators, dashboards, and local workspace persistence.

> **Status:** Early open-source development. APIs, storage formats, and UI behavior may change.

## Features

- Live OHLCV candle feeds with historical data and reconnect handling.
- Interactive candlestick charts with pan, zoom, crosshair, volume, and Heikin Ashi views.
- Technical indicators and exchange-aware price formatting.
- Dashboard modules for market data, news, fear and greed, order books, and clocks.
- Local-first dashboard and session persistence.
- Optional login: run and use the terminal offline/locally as a guest with full layout and settings persistence, without needing an account.
- Optional cloud account sync: when a compatible backend is configured, users can optionally sign in to sync dashboards and settings across devices.
- OS keyring integration for authenticated session storage, with a restricted local fallback when a keyring is unavailable.
- Verified HTTPS application updates using SHA-256 metadata checks.
- Cross-platform builds for Linux, macOS, and Windows.

## Requirements

- Rust stable toolchain with Cargo.
- Platform dependencies required by GPUI. On Ubuntu/Debian:

```sh
sudo apt-get update
sudo apt-get install -y \
  libxkbcommon-dev libxkbcommon-x11-dev \
  libwayland-dev libvulkan-dev \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libssl-dev pkg-config
```

## Build and test

```sh
cargo test --workspace
cargo fmt --check
cargo clippy -- -D warnings
cargo run --release
```

## Connecting to a Backend API (Optional)

FinTer is **local-first by default**: it runs offline in Guest Mode with local workspace and chart layout persistence, requiring no account or server. There is no pre-configured default API endpoint. If no endpoint is configured or market data cannot be fetched, guest sessions clearly display informative notifications and empty states across panels.

To connect to a custom backend or market gateway:
- **Via In-App Settings**: Go to **Settings → Network & Server**, and configure your API Server URL (e.g. `http://localhost:5173` for local development).
- **Via Environment Variable**: Set `FINTER_API_URL` before launching:
  ```sh
  FINTER_API_URL="http://localhost:5173" cargo run
  ```
- **Optional Gateway Overrides**:
  - `FINTER_REST_URL`: Override the REST market gateway origin (defaults to the API origin).
  - `FINTER_WS_URL`: Override the WebSocket market stream gateway.
  - `FINTER_NEWS_API_URL` & `FINTER_NEWS_WS_URL`: News article feed endpoints.

### Authentication & Token Security
When connecting to a backend service:
1. FinTer communicates strictly over TLS (HTTPS/WSS) or local loopback (`http://localhost:*`).
2. Authentication tokens (JWT `access_token` and `refresh_token`) are stored in the operating system's hardware credential vault via `keyring-rs` (Windows Credential Manager, macOS Keychain, Linux Secret Service).
3. Plaintext passwords are never persisted to disk.

### Data Separation (Guest vs. Cloud Session)
FinTer strictly isolates offline guest data from authenticated cloud data on disk:
- **Guest / Offline Session**:
  - Layout: `~/.config/finter/guest_layout.json`
  - Settings: `~/.config/finter/guest_settings.json`
  - Never synced to any remote server or overwritten by remote profiles.
- **Cloud Account Session**:
  - Layout: `~/.config/finter/cloud_layout.json` (cached locally, synchronized with database)
  - Settings: `~/.config/finter/cloud_settings.json`
  - Keyring Credential: OS Enclave (`finter/session`)
- **Seamless Switching**: Logging out immediately restores the guest workspace without erasing or altering your cloud configurations.

## Configuration and privacy

FinTer does not include API keys, application credentials, user data, session tokens, database dumps, compiled artifacts, or vendored dependency build directories. Authentication tokens are obtained at runtime and stored through the operating system keyring where available. The fallback session file is placed in the per-user configuration directory and is restricted to the current user on Unix systems.

Do not commit `.env` files, credentials, private keys, local session files, or generated build output. See [`.gitignore`](.gitignore).

## Release builds

Release automation is defined in [`.github/workflows`](.github/workflows). It runs tests before building platform binaries and produces SHA-256 checksums. Publishing update artifacts requires repository-hosted deployment secrets; those values must only be configured in GitHub Actions secrets and must never be committed to the repository.

Before publishing, review the workflow endpoints, artifact names, update metadata URL, and repository permissions for your deployment environment.

## Project layout

- `src/` — application, chart, UI, networking, authentication, and persistence code.
- `assets/` — application icons, fonts, and UI assets.
- `gpui-component-patched/` — the patched GPUI component dependency used by the application.
- `tests/updater/` — isolated updater verification tests.
- `scripts/` — release/publishing helpers.
- `docs/` — product and engineering documentation.

## License

FinTer is released under the [MIT License](LICENSE).
