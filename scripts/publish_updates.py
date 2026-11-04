"""Publish verified updater artifacts to public Backblaze B2 storage."""
import hashlib
import json
import os
from datetime import datetime, timezone
from pathlib import Path
import re
import subprocess
import tempfile
import tomllib
from urllib.parse import urlparse
from urllib.request import Request, urlopen


PUBLIC_USER_AGENT = "FinTer-Publisher/1.0"


def open_public(url: str):
    return urlopen(Request(url, headers={"User-Agent": PUBLIC_USER_AGENT}), timeout=120)


PLATFORMS = {
    "darwin-arm64": "finter",
    "darwin-x86": "finter",
    "windows": "finter.exe",
    "linux": "finter",
}


def obsolete_versions(versions: list[dict], channel: str, current_prefix: str) -> list[dict]:
    if channel not in {"alpha", "beta"} or not current_prefix.startswith(f"{channel}/"):
        raise ValueError("Pruning requires an alpha/beta build prefix")
    return [item for item in versions if item["Key"].startswith(f"{channel}/") and (
        (item["Key"] != f"{channel}/version.json" and not item["Key"].startswith(f"{current_prefix}/"))
        or not item["IsLatest"]
    )]


def prepare(root: Path, base_url: str, git_ref: str, git_sha: str):
    parsed = urlparse(base_url)
    if parsed.scheme != "https" or not parsed.netloc:
        raise ValueError("B2_PUBLIC_BASE_URL must be an HTTPS URL")
    version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError("Cargo version must be major.minor.patch")
    if git_ref in {"refs/heads/alpha", "refs/heads/beta"} and re.fullmatch(r"[0-9a-f]{40}", git_sha):
        channel = git_ref.removeprefix("refs/heads/")
        prefix = f"{channel}/{git_sha}"
    elif git_ref == f"refs/tags/v{version}":
        prefix, channel = f"releases/v{version}", "latest"
    else:
        raise ValueError("Expected alpha/beta ref or a v<version> tag matching Cargo.toml")

    metadata = {
        "version": version,
        "build_id": git_sha,
        "released_at": datetime.now(timezone.utc).isoformat(),
        "changelog": f"Build {git_sha}",
        "downloads": {},
        "checksums": {},
        "signatures": {},
    }
    files = []
    for platform, filename in PLATFORMS.items():
        path = root / "artifacts" / f"finter-{platform}" / filename
        checksum_path = Path(f"{path}.sha256")
        if not path.is_file() or not checksum_path.is_file():
            raise ValueError(f"Missing artifact: {platform}")
        with path.open("rb") as file:
            digest = hashlib.file_digest(file, "sha256").hexdigest()
        if path.stat().st_size == 0 or checksum_path.read_text().strip().lower() != digest:
            raise ValueError(f"Invalid checksum: {platform}")
        key = f"{prefix}/finter-{platform}" + (".exe" if platform == "windows" else "")
        metadata["downloads"][platform] = f"{base_url.rstrip('/')}/{key}"
        metadata["checksums"][platform] = digest
        files.append((path, key))

        # Optional detached GPG signature produced by the Linux signing step.
        signature = Path(f"{path}.asc")
        if signature.is_file() and signature.stat().st_size > 0:
            signature_key = f"{key}.asc"
            metadata["signatures"][platform] = f"{base_url.rstrip('/')}/{signature_key}"
            files.append((signature, signature_key))
    return metadata, files, prefix, channel


def main():
    required = ("AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_DEFAULT_REGION", "B2_ENDPOINT", "B2_BUCKET", "B2_PUBLIC_BASE_URL")
    missing = [name for name in required if not os.environ.get(name)]
    if missing:
        raise ValueError(f"Missing workflow values: {', '.join(missing)}")
    root = Path.cwd()
    metadata, files, prefix, channel = prepare(root, os.environ["B2_PUBLIC_BASE_URL"], os.environ["GITHUB_REF"], os.environ["GITHUB_SHA"])

    def aws(*args):
        return subprocess.run(
            ["aws", "--endpoint-url", os.environ["B2_ENDPOINT"], *args],
            check=True,
            capture_output=True,
            text=True,
        )

    def upload(path: Path, key: str, metadata_file=False):
        aws(
            "s3", "cp", str(path), f"s3://{os.environ['B2_BUCKET']}/{key}", "--only-show-errors",
            "--cache-control", "no-cache" if metadata_file else "public,max-age=31536000,immutable",
            "--content-type", "application/json" if metadata_file else "application/octet-stream",
        )

    def prune_channel():
        listing = json.loads(aws(
            "s3api", "list-object-versions", "--bucket", os.environ["B2_BUCKET"],
            "--prefix", f"{channel}/", "--output", "json",
        ).stdout)
        versions = listing.get("Versions", []) + listing.get("DeleteMarkers", [])
        for item in obsolete_versions(versions, channel, prefix):
            aws("s3api", "delete-object", "--bucket", os.environ["B2_BUCKET"],
                "--key", item["Key"], "--version-id", item["VersionId"])

    for path, key in files:
        upload(path, key)
    for platform, url in metadata["downloads"].items():
        with open_public(url) as response:
            if hashlib.file_digest(response, "sha256").hexdigest() != metadata["checksums"][platform]:
                raise ValueError(f"Public checksum mismatch: {platform}")
    with tempfile.TemporaryDirectory() as directory:
        metadata_path = Path(directory) / "version.json"
        metadata_path.write_text(json.dumps(metadata, indent=2) + "\n")
        upload(metadata_path, f"{prefix}/version.json", True)
        upload(metadata_path, f"{channel}/version.json", True)
    if channel in {"alpha", "beta"}:
        prune_channel()
    print(f"Published {channel}/version.json ({metadata['version']})")


if __name__ == "__main__":
    main()
