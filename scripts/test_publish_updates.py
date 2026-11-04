"""Run: python3 scripts/test_publish_updates.py"""
import hashlib
from pathlib import Path
import tempfile
from unittest.mock import patch
from publish_updates import open_public, prepare, obsolete_versions, PLATFORMS


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
print("Publication checks passed: alpha, beta, tag, HTTPS, version mismatch, checksum mismatch, signatures, pruning")
