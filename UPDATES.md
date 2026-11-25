# FinTer Update Support

Automatic update checks and downloads are disabled unless `FINTER_VERSION_URL` is configured at runtime or build time. FinTer does not contact the former CoinWatch update service by default. If you operate a compatible update service, ensure its metadata and artifact hosting are under your control before enabling this feature.

Updates require explicit user action to download and install. Downloads are bounded, require a SHA-256 checksum from metadata, and are reverified before installation. Installation replaces the current executable; restart remains user-controlled. SHA-256 detects corruption but does not authenticate metadata or publishers. Treat the configured HTTPS update host as trusted code distribution infrastructure; consider adding signed metadata before enabling updates for public releases.

## Chart order flow

The five separate order-flow toolbar buttons are now one **Order flow** menu with grouped footprint, profile, Delta/CVD, liquidity, L1, and analytics sections. Every toggle has one mutation path, shared by the menu and the `V`/`D`/`P`/`H`/`B` shortcuts; choosing a footprint mode no longer silently forces the footprint open.

Four pure analytics now render on the legacy chart path without touching the vendored chart crate:

- **Absorption** places a direction-coloured diamond on bars with top-decile volume but below-median range and a non-zero delta.
- **Delta divergence** draws a dashed line between consecutive price/CVD swing highs or lows plus a `BEAR`/`BULL` label.
- **Imbalance stacks** outline runs of at least three consecutive footprint imbalances at the bar's x position.
- **CVD regime** adds a `CVD RISING/FALLING/FLAT` badge to the Delta/CVD pane legend.

The analytics are cached by order-flow feed version (plus the row multiplier/imbalance ratio that shape stacks), not recomputed per frame, and never run on bars without real trade rows. The v2 engine still renders candles only; these overlays remain on the legacy path.

The footprint, Delta/CVD, volume profile, and liquidity overlays were tightened without changing the backend wire contract:

- Footprint rows now use the backend `row_size` (or the symbol tick / inferred source gaps) and are anchored to absolute prices, so panning and zooming cannot re-bucket prints. The footprint menu offers **Row size 1×/2×/4×/8×**; `1×` keeps backend rows 1:1, and merged rows split volume by price overlap.
- Value area expands contiguously from the POC, and ask/bid imbalances compare the correct diagonal neighbour with a positive opposite side required.
- Delta and CVD render in separate, independently scaled sub-panes with their own legends and a faint zero line. The volume profile follows the visible bars (VPVR); the backend session profile is used only when the visible range has no order-flow rows.
- The liquidity heatmap uses the visible time window and no longer clamps off-screen cells to the plot edge.
- Historical liquidity now backfills from CoinWatch's `GET /api/v1/orderbook/history` (change-only 1s L2 samples) behind a live ring bucketed to 1 second (the 10 Hz broadcast keeps the latest snapshot per bucket, so heatmap/L1 resolution matches history and the ring holds 15 minutes of each). The chart derives L1 best bid/ask from the same L2 top levels rather than spending a second ticker-history request. One pre-window (`carried`) sample seeds the left edge, and every heatmap cell is capped at 5s so the last historical sample cannot smear across the 1h lag seam. History pages fill up to 15 minutes behind the server's 1h-lagged cutoff; queued backfills retry five times, guest/401 shows `Sign in for historical depth`, and 402 surfaces the credit state. Layouts, settings, and credentials are never uploaded by this path.
- The new **L1** toolbar toggle (`B`) draws 1px best-bid/best-ask step lines from the same book feed over the candles. The step line breaks when consecutive samples are more than 5s apart, so the 1h history/live seam is not bridged by a long diagonal.

Order flow remains **live-session authoritative**. Historical candles do not fabricate footprint rows; the chart shows an `Order flow: live session` note when the viewport extends before the first live order-flow bar and no history has been loaded. The **Order flow → Load order-flow history** action replays CoinWatch's `GET /api/v1/trades/history` for the visible window (or the last 30 minutes when the chart is empty), aggregates the prints into exact-price `FlowBar`s, and merges only bars strictly older than the oldest live bar. The live seam bar is never duplicated; CVD and total delta are recomputed across the merged series. Raw trades are retained for 14 days and each load is capped at 12 pages × 20,000 rows (240,000 trades). The route is metered (3 credits) and only runs on explicit user action; guest/402/partial states surface as a note. Rendering changes are documented in `gpui-component-patched/FINTER-PATCHES.md`. No update/checksum behavior changed.

## Chart core (experimental, opt-in)

A scene-based chart engine now lives in `gpui-component-patched/src/chart/{core,scene,charts,elements,theme}.rs` and is selected with `FINTER_CHART_V2` (unset by default). The legacy `CandlestickChart` path remains the default and keeps drawings, alerts, positions/orders, footprint hover/pin, history paging and the L1/L2/liquidity-history overlays. The v2 path currently renders candles, volume, axes, grid, price scale and crosshair only. Layouts persist both the v1 and v2 price-scale fields, and the patch rationale is recorded in `gpui-component-patched/FINTER-PATCHES.md`.

## Release workflow

The included GitHub Actions workflow builds and tests platform binaries. On pushes to `alpha`, `beta`, or matching version tags (`v*`), it runs the `release` job to publish verified binaries to public Backblaze B2 using `scripts/publish_updates.py`.

Alpha/beta builds stage under `<channel>/<sha>/` and update `<channel>/version.json`, pruning obsolete builds. Tagged releases stage under `releases/v<version>/` and update `latest/version.json`. When the optional Linux GPG signing secrets are configured, detached `.asc` signatures are published next to the raw binaries and listed in the `signatures` map of `version.json`.

Publishing is idempotent and self-checking: each file is uploaded with a single `PutObject` (falling back to multipart above 5 GB), then confirmed in the bucket before metadata is written; a file that is already present with the expected size is skipped, and transient storage errors are retried with backoff. The workflow pins the AWS CLI's classic S3 transfer client with reduced concurrency because the CRT transfer client is unreliable against B2. If a publish fails after the build succeeded, re-run it without rebuilding through the **Publish** workflow (`workflow_dispatch`): supply the build run ID, the publishing ref (`refs/heads/alpha`, `refs/heads/beta`, or `refs/tags/v<version>`), and the build commit SHA for branch publishes.

CI embeds the matching metadata endpoint in each binary automatically: `alpha` → `https://data.coinwat.ch/alpha/version.json`, `beta` → `https://data.coinwat.ch/beta/version.json`, and `v*` tags → `https://data.coinwat.ch/latest/version.json`. Pull-request builds leave update checks disabled. Local builds can opt in at build or run time with `FINTER_VERSION_URL`.

Required GitHub Actions secrets:
- `B2_KEY_ID`: Application key ID.
- `B2_APPLICATION_KEY`: Application key secret.
- `B2_ENDPOINT`: S3 endpoint URL (e.g. `https://s3.us-west-004.backblazeb2.com`).
- `B2_REGION`: Bucket region (e.g. `us-west-004`).
- `B2_BUCKET`: Target B2 bucket name.
- `B2_PUBLIC_BASE_URL`: Public HTTPS origin serving the bucket files.

## Limits

- Supported targets in CI: macOS arm64/x86_64, Windows x86_64, Linux x86_64.
- macOS releases are signed with a Developer ID, hardened at runtime, notarized, and stapled. The `.app`/`.dmg` are distribution artifacts; the standalone `finter` binary remains the updater artifact. Replacing the binary inside a signed `.app` would invalidate its signature, so `.app` users should update by downloading the new signed `.dmg`.
- Windows releases are Authenticode-signed and timestamped. An EV certificate is recommended for immediate SmartScreen reputation.
- Linux AppImage artifacts are not self-updating; the updater replaces the raw `finter` binary, so use the published Linux binary rather than the AppImage. A `.tar.gz` fallback is produced for systems that restrict AppImage/FUSE.
- Updating an executable in a read-only/system install location may fail.
- SHA-256 is not a digital signature and does not protect against a compromised metadata host. Add signed update metadata before enabling public updates at scale.
- Release signing requires the repository secrets documented in [`docs/PLATFORM-SECURITY.md`](docs/PLATFORM-SECURITY.md). Missing credentials produce a warning by default; set the `REQUIRE_SIGNING` repository variable to `true` to make them a hard failure.

## Configuration

- `FINTER_VERSION_URL`: HTTPS URL to compatible `version.json` metadata; unset disables update checking. An explicit value always wins over the user-selected channel.
- `FINTER_BUILD_ID`: optional build identifier used to detect same-version channel updates.
- `FINTER_API_URL`: backend API origin for account/API features. Guest mode stays unconfigured; authenticated sessions default to the hosted staging origin.
- `FINTER_REST_URL`: optional REST market gateway; defaults to `FINTER_API_URL`.
- `FINTER_WS_URL`: optional WebSocket market gateway; otherwise derived from `FINTER_REST_URL`.
- `FINTER_NEWS_API_URL`: optional news API base URL.
- `FINTER_NEWS_WS_URL`: optional news WebSocket URL.
- `FINTER_CAP_URL`: optional market-cap snapshot URL; defaults to `<api-origin>/api/market-caps?limit=50`.
- `FINTER_CAP_WS_URL`: optional market-cap stream URL; defaults to `wss://<api-origin>/ws/market-caps`.

### Update channels

Users choose their release channel under **Settings → Updates** using the Stable, Beta, and Alpha buttons. Selecting a channel applies it immediately, checks that channel, and downloads and installs its current build. Switching channels may upgrade or downgrade the application; restart remains user-controlled. The selection is stored as `updateChannel` in the synced settings record and mirrored to `~/.finter/update_channel` so it applies before sign-in.

When the baked endpoint uses the official `<channel>/version.json` layout, FinTer rewrites the channel segment at runtime. Custom endpoints without an `alpha`, `beta`, or `latest` segment keep their configured URL. Explicit channel selection accepts the channel's current version even when it is older than the running build, while automatic checks continue to install only newer builds unless the selected channel differs from the binary's build channel.

Never commit credentials, tokens, or private deployment keys. Configure secrets through the hosting provider's secret store.

Content licenses for product name, logos, fonts, and bundled dependencies must be reviewed before redistribution.

**License:** This project's source license is MIT; bundled components and assets may have separate license terms.
