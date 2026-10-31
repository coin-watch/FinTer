# FinTer Update Support

Automatic update checks and downloads are disabled unless `FINTER_VERSION_URL` is configured at runtime or build time. FinTer does not contact the former CoinWatch update service by default. If you operate a compatible update service, ensure its metadata and artifact hosting are under your control before enabling this feature.

Updates require explicit user action to download and install. Downloads are bounded, require a SHA-256 checksum from metadata, and are reverified before installation. Installation replaces the current executable; restart remains user-controlled. SHA-256 detects corruption but does not authenticate metadata or publishers. Treat the configured HTTPS update host as trusted code distribution infrastructure; consider adding signed metadata before enabling updates for public releases.

## Release workflow

The included GitHub Actions workflow currently builds and tests platform binaries. Publishing is intentionally not configured with FinTer hosting credentials. Before enabling deployment, review the workflow's branch/tag triggers, configure only repository-scoped GitHub Actions secrets, and ensure artifact URLs and metadata are hosted under your control.

## Limits

- Supported targets in CI: macOS arm64/x86_64, Windows x86_64, Linux x86_64.
- macOS `.app` bundles are not built or notarized.
- Updating an executable in a read-only/system install location may fail.
- SHA-256 is not a digital signature and does not protect against a compromised metadata host.

## Configuration

- `FINTER_VERSION_URL`: HTTPS URL to compatible `version.json` metadata; unset disables update checking.
- `FINTER_BUILD_ID`: optional build identifier used to detect same-version channel updates.
- `FINTER_API_URL`: backend API origin for account/API features; empty by default (unconfigured).
- `FINTER_REST_URL`: optional REST market gateway; defaults to `FINTER_API_URL`.
- `FINTER_WS_URL`: optional WebSocket market gateway; otherwise derived from `FINTER_REST_URL`.
- `FINTER_NEWS_API_URL`: optional news API base URL.
- `FINTER_NEWS_WS_URL`: optional news WebSocket URL.

Never commit credentials, tokens, or private deployment keys. Configure secrets through the hosting provider's secret store.

Content licenses for product name, logos, fonts, and bundled dependencies must be reviewed before redistribution.

**License:** This project's source license is MIT; bundled components and assets may have separate license terms.
