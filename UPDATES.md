# FinTer Update Support

Automatic update checks and downloads are disabled unless `FINTER_VERSION_URL` is configured at runtime or build time. FinTer does not contact the former CoinWatch update service by default. If you operate a compatible update service, ensure its metadata and artifact hosting are under your control before enabling this feature.

Updates require explicit user action to download and install. Downloads are bounded, require a SHA-256 checksum from metadata, and are reverified before installation. Installation replaces the current executable; restart remains user-controlled. SHA-256 detects corruption but does not authenticate metadata or publishers. Treat the configured HTTPS update host as trusted code distribution infrastructure; consider adding signed metadata before enabling updates for public releases.

## Chart order flow removed

Order-flow functionality was removed from the chart and data layers. The chart no longer renders a footprint grid, Delta/CVD sub-panes, a volume profile, L1 best-bid/ask step lines, or a resting-liquidity heatmap, and the **Order flow** toolbar menu and its `V`/`D`/`P`/`H`/`B` shortcuts are gone. The derived `delta`/`footprint`/`profile` gateway subscriptions, the historical L2/L1 book routes, and the raw-trade history replay used only by those overlays were removed with their modules.

Existing layout files still load: the removed order-flow fields are simply ignored on read, and the `schema_version` and price-scale fields are unchanged, so no migration is required.

The standalone **Depth of Market** (order book) and **Time & Sales** (trade tape) panes are unaffected and continue to stream from the gateway's `orderbook` and `trades` subscriptions. The backend `GET /api/v1/trades/history` contract is unchanged; the desktop client no longer calls it.

## Chart controls moved to the pane title bar

Chart features no longer use the 34 px in-chart toolbar or the old in-chart indicator/color popovers. Each chart pane now shows four separate controls next to the timeframe buttons: **Type**, **Indicators**, **Drawings**, and **View**. The Type, Drawings, and View controls are dropdown menus; **Indicators** and **Drawing color** open transient floating OS windows bound to that pane.

The indicator window manages active studies, parameter editing with inline validation, visibility, removal, and adding new studies. The drawing-colour window owns the HSV square, hue strip, hex preview, and a single compact palette grid; the chosen colour applies to new drawings and updates the Drawings menu swatch. `I` opens the indicator window, and the existing `G`/`F`/`L` chart shortcuts continue to work, including on the opt-in `FINTER_CHART_V2` path.

The four title-bar triggers keep their full names in the tooltip and menu but use short labels on the bar (for example `Solid`, `Heikin`), and indicator category tags are neutral chips: colour stays reserved for market direction, armed tools and alerts.

Existing chart layout files still load unchanged. `library_geometry` remains in the layout model for compatibility but is no longer written or used by the chart panel.

## Chart core (experimental, opt-in)

A scene-based chart engine now lives in `gpui-component-patched/src/chart/{core,scene,charts,elements,theme}.rs` and is selected with `FINTER_CHART_V2` (unset by default). The legacy `CandlestickChart` path remains the default and keeps drawings, alerts, positions/orders, indicators, volume, and history paging. The v2 path currently renders candles, volume, axes, grid, price scale and crosshair only. Layouts persist both the v1 and v2 price-scale fields, and the patch rationale is recorded in `gpui-component-patched/FINTER-PATCHES.md`.

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
