"""Run: python3 scripts/test_publish_updates.py"""
import hashlib
from pathlib import Path
import tempfile
from unittest.mock import patch
from publish_updates import open_public, prepare, obsolete_versions, publishing_target, retry, upload_command, PLATFORMS, SINGLE_PUT_LIMIT


class Response:
    def __enter__(self): return self
    def __exit__(self, *_): pass


with patch("publish_updates.urlopen", return_value=Response()) as urlopen:
    open_public("https://example.com/file")
    assert urlopen.call_args.args[0].headers["User-agent"] == "FinTer-Publisher/1.0"

with tempfile.TemporaryDirectory() as directory:
    root = Path(directory)
    (root / "Cargo.toml").write_text('[package]\nversion = "0.1.0"\n')
    for platform, name in PLATFORMS.items():
        path = root / "artifacts" / f"finter-{platform}" / name
        path.parent.mkdir(parents=True)
        path.write_bytes(platform.encode())
        Path(f"{path}.sha256").write_text(hashlib.sha256(path.read_bytes()).hexdigest())
    sha = "a" * 40
    for ref, channel in [("refs/heads/alpha", "alpha"), ("refs/heads/beta", "beta"), ("refs/tags/v0.1.0", "latest")]:
        metadata, files, prefix, actual = prepare(root, "https://example.com", ref, sha)
        assert actual == channel and len(files) == len(metadata["downloads"]) == 4
        assert metadata["build_id"] == sha
        assert all(url.startswith(f"https://example.com/{prefix}/") for url in metadata["downloads"].values())
    signature = Path(f"{path}.asc")
    signature.write_text("-----BEGIN PGP SIGNATURE-----\n")
    metadata, files, prefix, _ = prepare(root, "https://example.com", "refs/heads/alpha", sha)
    assert metadata["signatures"]["linux"].endswith("/finter-linux.asc")
    assert any(key.endswith("/finter-linux.asc") for _, key in files)
    signature.unlink()
    for base, ref in [("http://example.com", "refs/heads/alpha"), ("https://example.com", "refs/tags/v9.0.0")]:
        try:
            prepare(root, base, ref, sha)
        except ValueError:
            pass
        else:
            raise AssertionError("Invalid input accepted")
    Path(f"{path}.sha256").write_text("0" * 64)
    try:
        prepare(root, "https://example.com", "refs/heads/alpha", sha)
    except ValueError:
        pass
    else:
        raise AssertionError("Corrupt artifact accepted")
versions = [
    {"Key": "alpha/version.json", "VersionId": "old-meta", "IsLatest": False},
    {"Key": "alpha/version.json", "VersionId": "new-meta", "IsLatest": True},
    {"Key": "alpha/aaa/finter-linux", "VersionId": "old-build", "IsLatest": True},
    {"Key": "alpha/bbb/finter-linux", "VersionId": "old-file", "IsLatest": False},
    {"Key": "alpha/bbb/finter-linux", "VersionId": "new-file", "IsLatest": True},
    {"Key": "beta/ccc/finter-linux", "VersionId": "beta-file", "IsLatest": True},
]
assert [item["VersionId"] for item in obsolete_versions(versions, "alpha", "alpha/bbb")] == ["old-meta", "old-build", "old-file"]
assert obsolete_versions(versions, "beta", "beta/ccc") == []

# retry: a transient failure is retried with exponential backoff, then succeeds.
calls = []
sleeps = []
def flaky():
    calls.append(1)
    if len(calls) < 2:
        raise RuntimeError("503 SlowDown")
    return "ok"
assert retry(flaky, "Uploading test", attempts=3, sleep=sleeps.append) == "ok"
assert calls == [1, 1] and sleeps == [2.0]

# retry: a permanent failure is surfaced after the attempts are exhausted.
def broken():
    raise RuntimeError("403 AccessDenied")
try:
    retry(broken, "Uploading test", attempts=3, sleep=lambda _: None)
except RuntimeError as error:
    assert "403 AccessDenied" in str(error)
else:
    raise AssertionError("Permanent failure was swallowed")

# publishing_target: GitHub values by default, explicit overrides for republishes.
assert publishing_target({"GITHUB_REF": "refs/heads/alpha", "GITHUB_SHA": "b" * 40}) == ("refs/heads/alpha", "b" * 40)
assert publishing_target({"PUBLISH_GIT_REF": "refs/heads/alpha", "PUBLISH_GIT_SHA": "c" * 40,
                          "GITHUB_REF": "refs/heads/main", "GITHUB_SHA": "d" * 40}) == ("refs/heads/alpha", "c" * 40)
assert publishing_target({"PUBLISH_GIT_REF": "refs/tags/v0.2.0"}) == ("refs/tags/v0.2.0", "")
try:
    publishing_target({"PUBLISH_GIT_REF": "refs/heads/alpha"})
except ValueError as error:
    assert "PUBLISH_GIT_SHA" in str(error)
else:
    raise AssertionError("Branch republish without a SHA was accepted")

# upload_command: single PutObject for normal artifacts, multipart above the B2 limit.
binary = Path("finter-windows.exe")
command = upload_command(300 * 1024 ** 2, binary, "bucket", "alpha/build/finter-windows.exe", False)
assert command[:2] == ["s3api", "put-object"]
assert "application/octet-stream" in command and "public,max-age=31536000,immutable" in command
metadata_command = upload_command(512, Path("version.json"), "bucket", "alpha/version.json", True)
assert metadata_command[:2] == ["s3api", "put-object"]
assert "application/json" in metadata_command and "no-cache" in metadata_command
huge = upload_command(SINGLE_PUT_LIMIT + 1, binary, "bucket", "key", False)
assert huge[:2] == ["s3", "cp"] and "s3://bucket/key" in huge

print("Publication checks passed: alpha, beta, tag, HTTPS, version mismatch, checksum mismatch, signatures, pruning, retries, republish targets, upload commands")
