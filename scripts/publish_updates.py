"""Publish verified updater artifacts to public Backblaze B2 storage."""
import hashlib
import json
import os
from datetime import datetime, timezone
from pathlib import Path
import re
import subprocess
import tempfile
import time
import tomllib
from urllib import error as urlerror
from urllib.parse import urlparse
from urllib.request import Request, urlopen


PUBLIC_USER_AGENT = "FinTer-Publisher/1.0"

# B2 accepts single PutObject uploads up to 5 GB; larger objects need multipart.
SINGLE_PUT_LIMIT = 5 * 1024 ** 3


def open_public(url: str):
    return urlopen(Request(url, headers={"User-Agent": PUBLIC_USER_AGENT}), timeout=120)


def retry(operation, description: str, attempts: int = 3, delay: float = 2.0, sleep=time.sleep):
    """Run an operation, retrying transient failures with exponential backoff."""
    for attempt in range(1, attempts + 1):
        try:
            return operation()
        except RuntimeError as error:
            if attempt == attempts:
                raise
            wait = delay * (2 ** (attempt - 1))
            print(f"{description} failed (attempt {attempt}/{attempts}): {error}", flush=True)
            print(f"Retrying in {wait:g}s", flush=True)
            sleep(wait)


def upload_command(size: int, path: Path, bucket: str, key: str, metadata_file: bool) -> list[str]:
    """Build the aws CLI arguments that upload one file to B2."""
    cache_control = "no-cache" if metadata_file else "public,max-age=31536000,immutable"
    content_type = "application/json" if metadata_file else "application/octet-stream"
    if size <= SINGLE_PUT_LIMIT:
        # A single PutObject avoids the multipart and transfer-manager code paths
        # that have intermittently reported success without publishing the object.
        return [
            "s3api", "put-object", "--bucket", bucket, "--key", key, "--body", str(path),
            "--cache-control", cache_control, "--content-type", content_type,
        ]
    return [
        "s3", "cp", str(path), f"s3://{bucket}/{key}", "--only-show-errors",
        "--cache-control", cache_control, "--content-type", content_type,
    ]


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


def publishing_target(environ):
    """Return the ref/sha to publish, allowing explicit overrides for republishes."""
    if environ.get("PUBLISH_GIT_REF"):
        ref = environ["PUBLISH_GIT_REF"]
        sha = environ.get("PUBLISH_GIT_SHA", "")
        if not sha and not ref.startswith("refs/tags/"):
            raise ValueError("PUBLISH_GIT_SHA is required when PUBLISH_GIT_REF is a branch")
        return ref, sha
    return environ["GITHUB_REF"], environ["GITHUB_SHA"]


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
    git_ref, git_sha = publishing_target(os.environ)
    metadata, files, prefix, channel = prepare(root, os.environ["B2_PUBLIC_BASE_URL"], git_ref, git_sha)

    version = subprocess.run(["aws", "--version"], capture_output=True, text=True)
    if version.returncode == 0:
        print((version.stdout or version.stderr).strip(), flush=True)

    def aws(*args):
        command = ["aws", "--endpoint-url", os.environ["B2_ENDPOINT"], *args]
        result = subprocess.run(command, capture_output=True, text=True)
        if result.returncode != 0:
            detail = (result.stderr or result.stdout or "").strip()
            raise RuntimeError(f"`{' '.join(command)}` exited {result.returncode}: {detail}")
        if result.stderr.strip():
            print(result.stderr.strip(), flush=True)
        return result

    def object_size(key: str):
        """Return (known, size); known is False when the lookup itself failed."""
        result = subprocess.run(
            ["aws", "--endpoint-url", os.environ["B2_ENDPOINT"], "s3api", "head-object",
             "--bucket", os.environ["B2_BUCKET"], "--key", key],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            detail = result.stderr.strip()
            if "(404)" in detail or "Not Found" in detail or "NoSuchKey" in detail:
                return True, None
            print(f"Could not inspect {key}: {detail}", flush=True)
            return False, None
        try:
            return True, int(json.loads(result.stdout)["ContentLength"])
        except (ValueError, TypeError, KeyError):
            return False, None

    def upload(path: Path, key: str, metadata_file=False):
        size = path.stat().st_size
        # Object keys are immutable per build, so a file that is already present
        # with the expected size was published by an earlier (possibly partial)
        # run. The post-upload checksum verification still guards the content.
        if not metadata_file:
            known, published = object_size(key)
            if known and published == size:
                print(f"Skipping {key}: already published ({size} bytes)", flush=True)
                return

        def publish():
            aws(*upload_command(size, path, os.environ["B2_BUCKET"], key, metadata_file))
            known, published = object_size(key)
            if known and published != size:
                raise RuntimeError(f"object is {published!r} bytes after upload, expected {size}")

        retry(publish, f"Uploading {key} ({size} bytes)")
        print(f"Uploaded {key} ({size} bytes)", flush=True)

    def dump_prefix():
        try:
            listing = json.loads(aws(
                "s3api", "list-objects-v2", "--bucket", os.environ["B2_BUCKET"],
                "--prefix", f"{prefix}/", "--output", "json",
            ).stdout).get("Contents", [])
        except (RuntimeError, ValueError) as error:
            print(f"Could not list {prefix}/: {error}", flush=True)
            return
        print(f"Objects published under {prefix}/:", flush=True)
        for item in listing:
            print(f"  {item['Key']} ({item['Size']} bytes)", flush=True)

    def verify_public(metadata):
        for platform, url in metadata["downloads"].items():
            expected = metadata["checksums"][platform]
            for attempt in range(1, 4):
                try:
                    with open_public(url) as response:
                        digest = hashlib.file_digest(response, "sha256").hexdigest()
                except (urlerror.URLError, OSError) as error:
                    failure = error
                else:
                    if digest != expected:
                        raise ValueError(f"Public checksum mismatch: {platform} ({url})")
                    print(f"Verified {platform}: {url}", flush=True)
                    break
                if attempt == 3:
                    dump_prefix()
                    raise RuntimeError(f"Public verification failed for {platform} ({url}): {failure}")
                wait = 3 * attempt
                print(f"Verifying {platform} failed (attempt {attempt}/3): {failure}", flush=True)
                print(f"Retrying in {wait}s", flush=True)
                time.sleep(wait)

    def prune_channel():
        listing = json.loads(retry(
            lambda: aws(
                "s3api", "list-object-versions", "--bucket", os.environ["B2_BUCKET"],
                "--prefix", f"{channel}/", "--output", "json",
            ),
            "Listing channel versions",
        ).stdout)
        versions = listing.get("Versions", []) + listing.get("DeleteMarkers", [])
        for item in obsolete_versions(versions, channel, prefix):
            retry(
                lambda: aws("s3api", "delete-object", "--bucket", os.environ["B2_BUCKET"],
                            "--key", item["Key"], "--version-id", item["VersionId"]),
                f"Deleting {item['Key']} ({item['VersionId']})",
            )

    for path, key in files:
        upload(path, key)
    verify_public(metadata)
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
