# Platform security and release signing

This document describes how FinTer builds avoid being blocked, quarantined, or
flagged by the security systems of the platforms it ships on. It also lists the
repository secrets that are required for signed release builds.

The build workflow uses the following policy:

- Missing macOS or Windows signing credentials produce a **warning** and the
  build continues unsigned, so forks and staging environments are not blocked.
- Set the `REQUIRE_SIGNING` repository variable to `true` to turn missing
  credentials into a **hard failure**. Enable this once the production secrets
  are configured.
- Pull-request builds are never blocked by the signing policy.

## Why each platform blocks unsigned software

| Platform | Gate | What it does to unsigned builds |
| --- | --- | --- |
| macOS | Gatekeeper | Blocks apps downloaded from the internet with "Apple cannot check it for malicious software". |
| macOS | Notarization | Scans Developer ID software for malicious components; without a ticket Gatekeeper warns or blocks. |
| Windows | SmartScreen | Shows "Windows protected your PC" for binaries without reputation or a trusted signature. |
| Windows | Smart App Control / Defender | May block or quarantine unsigned, low-reputation executables. |
| Linux | AppImage / FUSE | Some distributions restrict unprivileged user namespaces or FUSE, preventing an AppImage from mounting. |

## macOS

### What the build produces

- `finter` — the standalone updater artifact. Signed with a Developer ID and
  hardened runtime. It is notarized as part of the app bundle submission.
- `FinTer.app.zip` — the signed, notarized, and stapled app bundle.
- `FinTer-<target>.dmg` — a signed, notarized, and stapled disk image.

### Required repository secrets

| Secret | Purpose |
| --- | --- |
| `MACOS_CERTIFICATE` | Base64-encoded Developer ID Application `.p12` certificate. |
| `MACOS_CERTIFICATE_PASSWORD` | Password for the `.p12` file. |
| `MACOS_KEYCHAIN_PASSWORD` | Temporary keychain password used only on the runner. |
| `MACOS_SIGNING_IDENTITY` | Full identity, e.g. `Developer ID Application: CoinWatch (TEAMID)`. |
| `MACOS_NOTARY_APPLE_ID` | Apple ID used for notarization. |
| `MACOS_NOTARY_PASSWORD` | App-specific password for that Apple ID. |
| `MACOS_TEAM_ID` | Apple Developer team ID. |

Create the certificate in the Apple Developer portal as **Developer ID
Application**, export it as `.p12`, and base64-encode it without line wrapping:

```sh
base64 -w0 developer-id-application.p12 > certificate.b64
```

Store the contents of `certificate.b64` in `MACOS_CERTIFICATE`.

### What the workflow does

1. Builds the release binary.
2. Creates a `FinTer.app` bundle with `Contents/Info.plist` and the `.icns`
   icon.
3. Imports the Developer ID certificate into a temporary keychain.
4. Signs the app bundle with
   `--options runtime --timestamp --entitlements assets/platform/macos/FinTer.entitlements`,
   then copies the signed main executable back to the standalone updater
   artifact so both share the same signature.
5. Verifies the signatures with `codesign --verify --deep --strict`.
6. Submits the app to `notarytool`, staples the ticket, and validates it with
   `stapler` and `spctl`.
7. Creates a DMG, signs it, notarizes it, and staples it.

### Verifying a downloaded build

```sh
codesign --verify --deep --strict --verbose=2 FinTer.app
spctl --assess --type execute --verbose=4 FinTer.app
xcrun stapler validate FinTer.app
xcrun stapler validate FinTer-<target>.dmg
```

### Updater caveat

The in-app updater replaces the running executable. Replacing the binary inside
a signed `.app` bundle invalidates the bundle signature and its notarization
ticket. macOS users who install the `.app` or `.dmg` should update by
downloading the new signed `.dmg`. The standalone `finter` binary remains the
updater artifact for users who run the raw binary directly.

## Windows

### What the build produces

- `finter.exe` — the release binary and updater artifact, signed with
  Authenticode and timestamped.

### Required repository secrets

| Secret | Purpose |
| --- | --- |
| `WINDOWS_CERTIFICATE` | Base64-encoded Authenticode `.pfx` certificate. |
| `WINDOWS_CERTIFICATE_PASSWORD` | Password for the `.pfx` file. |
| `WINDOWS_TIMESTAMP_URL` | Optional RFC 3161 timestamp URL. Defaults to `http://timestamp.digicert.com`. |

An **EV code-signing certificate** is strongly recommended. EV certificates
grant SmartScreen reputation immediately; standard OV certificates accumulate
reputation over time. Store the certificate in a hardware token or HSM where
possible, and export a CI-only `.pfx` only if your certificate policy allows it.

Base64-encode the `.pfx` without line wrapping and store it in
`WINDOWS_CERTIFICATE`:

```powershell
[Convert]::ToBase64String([IO.File]::ReadAllBytes("authenticode.pfx")) |
  Set-Content -NoNewline certificate.b64
```

If your certificate policy does not allow exporting a `.pfx`, use Azure Trusted
Signing or a cloud HSM and replace the `signtool sign` invocation with the
provider's signing command. Keep the `signtool verify /pa` check.

### What the workflow does

1. Embeds an application manifest with `asInvoker`, Windows 10/11
   compatibility, per-monitor DPI awareness, long-path support, and UTF-8 as
   the active code page.
2. Embeds `VERSIONINFO` metadata (product name, company, version, original
   filename).
3. Signs `finter.exe` with `signtool sign /fd SHA256 /td SHA256 /tr <timestamp>`.
4. Verifies the signature with `signtool verify /pa /v`.
5. Generates the SHA-256 checksum **after** signing, so the published checksum
   matches the signed bytes.

### Verifying a downloaded build

```powershell
Get-AuthenticodeSignature .\finter.exe | Format-List
signtool verify /pa /v .\finter.exe
```

### Microsoft Defender false positives

Self-updating applications that download and replace their own executable can
be flagged by heuristics even when signed. If Defender or another engine flags
a release:

1. Confirm the binary is signed and timestamped.
2. Submit the file to Microsoft at <https://www.microsoft.com/wdsi/filesubmission>
   as a software developer false positive.
3. Keep the `VERSIONINFO` metadata and Authenticode signature intact; do not
   pack or obfuscate the binary.

## Linux

### What the build produces

- `finter` — the raw binary and updater artifact.
- `finter-<target>.AppImage` — the desktop AppImage with AppStream metadata.
- `finter-<target>.tar.gz` — a fallback archive for systems where AppImage or
  FUSE is restricted.

### AppStream metadata

The AppImage ships `usr/share/metainfo/ch.coinwat.finter.appdata.xml`. Software centers
use it to show the application name, developer, license, screenshots, and
release notes instead of treating the AppImage as an anonymous executable. The
workflow validates the metadata with `appstreamcli validate --no-net` when the
tool is available.

### Optional GPG signatures

| Secret | Purpose |
| --- | --- |
| `APPIMAGE_GPG_PRIVATE_KEY` | Base64-encoded ASCII-armored GPG private key. |
| `APPIMAGE_GPG_KEY_ID` | Key ID or fingerprint used for signing. |
| `APPIMAGE_GPG_PASSPHRASE` | Optional key passphrase. |

When configured, the workflow creates detached `.asc` signatures for the raw
binary, the AppImage, and the tarball, and verifies each signature immediately.
Linux does not enforce a platform-wide signature requirement, so these secrets
are optional. They are still recommended for users who verify downloads.

### FUSE fallback

Some distributions restrict unprivileged user namespaces or FUSE, which can
prevent a type-2 AppImage from mounting. The `.tar.gz` artifact contains the
binary, desktop file, icon, and AppStream metadata so users can install or run
FinTer without the AppImage runtime.

### Verifying a downloaded build

```sh
sha256sum -c finter-<target>.AppImage.sha256
gpg --verify finter-<target>.AppImage.asc finter-<target>.AppImage
appstreamcli validate ch.coinwat.finter.appdata.xml
```

## CI supply-chain hardening

- The workflow declares `permissions: contents: read` and disables persisted
  checkout credentials.
- GitHub Actions are pinned to commit SHAs.
- The AppImage packaging tool is pinned to release `1.9.1` and verified against
  a SHA-256 checksum before execution. The upstream `continuous` tag is
  mutable and is not used.
- Signing secrets are injected only into the steps that need them.
- The Windows checksum is generated after Authenticode signing.

## Update metadata authenticity

The updater verifies SHA-256 checksums from `version.json` over HTTPS. SHA-256
detects corruption but does not authenticate the publisher: a compromised
metadata host could publish a different binary and matching checksum. Before
enabling public updates at scale, add a detached signature over `version.json`
(for example Ed25519 or minisign) and verify it in the client before trusting
the download URLs or checksums.

## Secret rotation

- Rotate the macOS Developer ID certificate before it expires and update
  `MACOS_CERTIFICATE`, `MACOS_CERTIFICATE_PASSWORD`, and
  `MACOS_SIGNING_IDENTITY` together.
- Rotate the Windows Authenticode certificate before it expires and update
  `WINDOWS_CERTIFICATE` and `WINDOWS_CERTIFICATE_PASSWORD`.
- Rotate the GPG signing key and update `APPIMAGE_GPG_PRIVATE_KEY` and
  `APPIMAGE_GPG_KEY_ID`.
- Never commit certificates, private keys, passwords, or `.p12`/`.pfx` files.
