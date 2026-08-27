#!/usr/bin/env python3
"""Verify configured public URLs and optional pinned SHA-256 digests."""

from __future__ import annotations

import argparse
import concurrent.futures
import dataclasses
import datetime as dt
import hashlib
import json
import re
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path


USER_AGENT = "AnnoCAT-source-contract/1"
RETRY_STATUSES = {408, 429, 500, 502, 503, 504}
SERVICE_STATUSES = {400, 405, 415, 422}
ADVISORY_KEYS = {
    "hgncReleaseUrl",
    "providerUrl",
    "referenceUrl",
    "releaseUrl",
    "upstreamRepository",
}


@dataclasses.dataclass
class Target:
    url: str
    kind: str
    locations: list[str]
    expected_bytes: int | None = None
    expected_sha256: str | None = None


def expected_size(parent: dict, key: str, url: str) -> int | None:
    size_key = {
        "archiveUrl": "archiveBytes",
        "dataUrl": "dataBytes",
        "indexUrl": "indexBytes",
        "url": "bytes" if "bytes" in parent else "compressedBytes",
    }.get(key)
    if key == "primaryUrl" and not url.endswith(("/", "/releases/latest")):
        if "/records/" not in url:
            size_key = "downloadBytes"
    value = parent.get(size_key) if size_key else None
    return value if isinstance(value, int) and value > 0 else None


def expected_sha256(parent: dict, key: str) -> str | None:
    value = parent.get("sha256") if key == "url" else None
    if value is None:
        return None
    if not isinstance(value, str) or re.fullmatch(r"[0-9a-fA-F]{64}", value) is None:
        raise ValueError("configured SHA-256 must contain exactly 64 hexadecimal characters")
    return value.lower()


def classify(filename: str, key: str, url: str) -> str:
    if filename == "evidence-calibrations.json" or key in ADVISORY_KEYS:
        return "advisory"
    if key in {"apiUrl", "codingApiUrl"}:
        return "service"
    if key == "primaryUrl" and url.endswith("/"):
        return "prefix"
    if key == "primaryUrl" and url.endswith("/releases/latest"):
        return "advisory"
    return "required"


def iter_urls(value, path=()):
    if isinstance(value, dict):
        for key, child in value.items():
            child_path = (*path, key)
            if isinstance(child, str) and child.startswith(("https://", "http://")):
                yield child_path, key, child, value
            else:
                yield from iter_urls(child, child_path)
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from iter_urls(child, (*path, index))


def path_label(filename: str, path: tuple) -> str:
    suffix = ""
    for part in path:
        suffix += f"[{part}]" if isinstance(part, int) else f".{part}"
    return f"{filename}{suffix}"


def load_targets(config_dir: Path) -> tuple[list[Target], int]:
    merged: dict[str, Target] = {}
    skipped = 0
    precedence = {"advisory": 0, "prefix": 1, "service": 2, "required": 3}
    for path in sorted(config_dir.glob("*.json")):
        if path.name == "source-overrides.example.json":
            skipped += 1
            continue
        with path.open(encoding="utf-8") as handle:
            document = json.load(handle)
        for json_path, key, url, parent in iter_urls(document):
            location = path_label(path.name, json_path)
            kind = classify(path.name, key, url)
            size = expected_size(parent, key, url)
            sha256 = expected_sha256(parent, key)
            current = merged.get(url)
            if current is None:
                merged[url] = Target(url, kind, [location], size, sha256)
                continue
            current.locations.append(location)
            if precedence[kind] > precedence[current.kind]:
                current.kind = kind
            if size is not None:
                if current.expected_bytes not in (None, size):
                    raise ValueError(f"conflicting expected sizes for {url}")
                current.expected_bytes = size
            if sha256 is not None:
                if current.expected_sha256 not in (None, sha256):
                    raise ValueError(f"conflicting SHA-256 values for {url}")
                current.expected_sha256 = sha256
    return sorted(merged.values(), key=lambda item: item.url), skipped


def one_request(url: str, method: str, timeout: float):
    headers = {
        "User-Agent": USER_AGENT,
        "Accept": "*/*",
        "Accept-Encoding": "identity",
    }
    if method == "GET":
        headers["Range"] = "bytes=0-0"
    request = urllib.request.Request(url, headers=headers, method=method)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            if method == "GET":
                response.read(1)
            return response.status, response.geturl(), response.headers, ""
    except urllib.error.HTTPError as error:
        return error.code, error.geturl(), error.headers, str(error)
    except (OSError, urllib.error.URLError) as error:
        return 0, url, {}, str(error)


def request_with_retries(url: str, method: str, timeout: float):
    result = None
    for attempt in range(3):
        result = one_request(url, method, timeout)
        if result[0] not in RETRY_STATUSES and result[0] != 0:
            return result
        if attempt < 2:
            time.sleep(0.5 * (attempt + 1))
    return result


def response_size(headers) -> int | None:
    content_range = headers.get("Content-Range", "")
    match = re.search(r"/(\d+)$", content_range)
    if match:
        return int(match.group(1))
    content_length = headers.get("Content-Length")
    return int(content_length) if content_length and content_length.isdigit() else None


def check_target(target: Target, timeout: float) -> dict:
    status, final_url, headers, error = request_with_retries(target.url, "HEAD", timeout)
    method = "HEAD"
    acceptable = 200 <= status < 400 or (
        target.kind == "service" and status in SERVICE_STATUSES
    )
    head_size = response_size(headers)
    size_needs_confirmation = (
        target.expected_bytes is not None and head_size != target.expected_bytes
    )
    if target.kind != "service" and (not acceptable or size_needs_confirmation):
        status, final_url, headers, error = request_with_retries(target.url, "GET", timeout)
        method = "GET range"
        acceptable = 200 <= status < 400
    actual_bytes = response_size(headers)
    size_ok = target.expected_bytes is None or actual_bytes == target.expected_bytes
    return {
        "url": target.url,
        "kind": target.kind,
        "locations": sorted(target.locations),
        "method": method,
        "httpStatus": status or None,
        "finalUrl": final_url,
        "expectedBytes": target.expected_bytes,
        "actualBytes": actual_bytes,
        "expectedSha256": target.expected_sha256,
        "actualSha256": None,
        "sha256Verified": None,
        "ok": acceptable and size_ok,
        "error": "" if acceptable and size_ok else (
            f"expected {target.expected_bytes} bytes, received {actual_bytes}"
            if acceptable and not size_ok
            else error or f"HTTP {status}"
        ),
    }


def stream_sha256(target: Target, timeout: float) -> dict:
    headers = {
        "User-Agent": USER_AGENT,
        "Accept": "*/*",
        "Accept-Encoding": "identity",
    }
    request = urllib.request.Request(target.url, headers=headers, method="GET")
    last_error = ""
    for attempt in range(3):
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                digest = hashlib.sha256()
                actual_bytes = 0
                while chunk := response.read(1024 * 1024):
                    digest.update(chunk)
                    actual_bytes += len(chunk)
                actual_sha256 = digest.hexdigest()
                size_ok = (
                    target.expected_bytes is None
                    or actual_bytes == target.expected_bytes
                )
                digest_ok = actual_sha256 == target.expected_sha256
                return {
                    "actualBytes": actual_bytes,
                    "actualSha256": actual_sha256,
                    "sha256Verified": size_ok and digest_ok,
                    "error": "" if size_ok and digest_ok else (
                        f"expected {target.expected_bytes} bytes, received {actual_bytes}"
                        if not size_ok
                        else f"SHA-256 mismatch: received {actual_sha256}"
                    ),
                }
        except urllib.error.HTTPError as error:
            last_error = str(error)
            if error.code not in RETRY_STATUSES:
                break
        except (OSError, urllib.error.URLError) as error:
            last_error = str(error)
        if attempt < 2:
            time.sleep(0.5 * (attempt + 1))
    return {
        "actualSha256": None,
        "sha256Verified": False,
        "error": last_error or "cannot download asset for SHA-256 verification",
    }


def apply_sha256_results(
    targets: list[Target], results: list[dict], timeout: float, workers: int
) -> None:
    digest_targets = [target for target in targets if target.expected_sha256 is not None]
    if not digest_targets:
        return
    with concurrent.futures.ThreadPoolExecutor(max_workers=min(workers, 4)) as executor:
        checks = executor.map(lambda target: stream_sha256(target, timeout), digest_targets)
        by_url = {result["url"]: result for result in results}
        for target, check in zip(digest_targets, checks):
            result = by_url[target.url]
            result.update(check)
            result["ok"] = check["sha256Verified"]


def apply_prefix_results(targets: list[Target], results: list[dict]) -> None:
    by_url = {result["url"]: result for result in results}
    for target in targets:
        if target.kind != "prefix":
            continue
        children = [
            result
            for url, result in by_url.items()
            if url != target.url and url.startswith(target.url) and result["kind"] == "required"
        ]
        by_url[target.url] = {
            "url": target.url,
            "kind": "prefix",
            "locations": sorted(target.locations),
            "method": "covered by configured child assets",
            "httpStatus": None,
            "finalUrl": target.url,
            "expectedBytes": None,
            "actualBytes": None,
            "expectedSha256": None,
            "actualSha256": None,
            "sha256Verified": None,
            "ok": bool(children) and all(child["ok"] for child in children),
            "error": "" if children and all(child["ok"] for child in children)
            else "no complete reachable child-asset set uses this prefix",
        }
    results[:] = sorted(by_url.values(), key=lambda item: item["url"])


def self_test() -> None:
    assert classify("evidence-calibrations.json", "referenceUrl", "https://x") == "advisory"
    assert classify("source-catalog.json", "apiUrl", "https://x") == "service"
    assert classify("source-catalog.json", "primaryUrl", "https://x/") == "prefix"
    assert classify("hpo-assets.json", "url", "https://x/file") == "required"
    assert expected_size({"url": "x", "bytes": 12}, "url", "https://x") == 12
    assert expected_size({"dataBytes": 34}, "dataUrl", "https://x") == 34
    assert expected_sha256({"sha256": "a" * 64}, "url") == "a" * 64
    assert expected_sha256({"sha256": "a" * 64}, "primaryUrl") is None
    assert response_size({"Content-Range": "bytes 0-0/56"}) == 56
    assert response_size({"Content-Length": "78"}) == 78


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--config-dir",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "config",
    )
    parser.add_argument("--report", type=Path)
    parser.add_argument("--workers", type=int, default=12)
    parser.add_argument("--timeout", type=float, default=20)
    parser.add_argument(
        "--verify-sha256",
        action="store_true",
        help="stream assets with an explicit SHA-256 and verify their full contents",
    )
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        print("configured URL validator self-test passed")
        return 0

    targets, skipped = load_targets(args.config_dir)
    prefixes = [target for target in targets if target.kind == "prefix"]
    checked = [target for target in targets if target.kind != "prefix"]
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as executor:
        results = list(executor.map(lambda target: check_target(target, args.timeout), checked))
    results.extend(
        {
            "url": target.url,
            "kind": target.kind,
            "locations": target.locations,
            "ok": False,
        }
        for target in prefixes
    )
    if args.verify_sha256:
        apply_sha256_results(targets, results, args.timeout, args.workers)
    apply_prefix_results(targets, results)

    blocking = [result for result in results if not result["ok"] and result["kind"] != "advisory"]
    advisory = [result for result in results if not result["ok"] and result["kind"] == "advisory"]
    report = {
        "schemaVersion": 1,
        "generatedAt": dt.datetime.now(dt.timezone.utc).isoformat(),
        "summary": {
            "configuredUrls": len(results),
            "blockingFailures": len(blocking),
            "advisoryFailures": len(advisory),
            "sha256Verified": sum(
                result.get("sha256Verified") is True for result in results
            ),
            "skippedExampleFiles": skipped,
        },
        "targets": results,
    }
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    print(
        f"checked {len(results)} configured URLs: "
        f"{len(blocking)} blocking failure(s), {len(advisory)} advisory failure(s), "
        f"{report['summary']['sha256Verified']} SHA-256 digest(s) verified"
    )
    for result in (*blocking, *advisory):
        level = "ERROR" if result in blocking else "WARNING"
        print(f"{level}: {result['url']}: {result.get('error', '')}", file=sys.stderr)
    return 1 if blocking else 0


if __name__ == "__main__":
    raise SystemExit(main())
