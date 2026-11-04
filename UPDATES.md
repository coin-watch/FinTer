# FinTer Update Support

Automatic update checks and downloads are disabled unless `FINTER_VERSION_URL` is configured at runtime or build time. FinTer does not contact the former CoinWatch update service by default. If you operate a compatible update service, ensure its metadata and artifact hosting are under your control before enabling this feature.

Updates require explicit user action to download and install. Downloads are bounded, require a SHA-256 checksum from metadata, and are reverified before installation. Installation replaces the current executable; restart remains user-controlled. SHA-256 detects corruption but does not authenticate metadata or publishers. Treat the configured HTTPS update host as trusted code distribution infrastructure; consider adding signed metadata before enabling updates for public releases.

## Release workflow

The included GitHub Actions workflow builds and tests platform binaries. On pushes to `alpha`, `beta`, or matching version tags (`v*`), it runs the `release` job to publish verified binaries to public Backblaze B2 using `scripts/publish_updates.py`.

Alpha/beta builds stage under `<channel>/<sha>/` and update `<channel>/version.json`, pruning obsolete builds. Tagged releases stage under `releases/v<version>/` and update `latest/version.json`. When the optional Linux GPG signing secrets are configured, detached `.asc` signatures are published next to the raw binaries and listed in the `signatures` map of `version.json`.

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
- Release signing requires the repository secrets documented in [`docs/PLATFORM-SECURITY.md`](docs/PLATFORM-SECURITY.md). Release builds fail when required signing credentials are missing unless `ALLOW_UNSIGNED_RELEASE` is set.

## Configuration

- `FINTER_VERSION_URL`: HTTPS URL to compatible `version.json` metadata; unset disables update checking.
- `FINTER_BUILD_ID`: optional build identifier used to detect same-version channel updates.
- `FINTER_API_URL`: backend API origin for account/API features. Guest mode stays unconfigured; authenticated sessions default to the hosted staging origin.
- `FINTER_REST_URL`: optional REST market gateway; defaults to `FINTER_API_URL`.
- `FINTER_WS_URL`: optional WebSocket market gateway; otherwise derived from `FINTER_REST_URL`.
- `FINTER_NEWS_API_URL`: optional news API base URL.
- `FINTER_NEWS_WS_URL`: optional news WebSocket URL.

Never commit credentials, tokens, or private deployment keys. Configure secrets through the hosting provider's secret store.

Content licenses for product name, logos, fonts, and bundled dependencies must be reviewed before redistribution.

**License:** This project's source license is MIT; bundled components and assets may have separate license terms.
