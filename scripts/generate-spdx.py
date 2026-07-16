#!/usr/bin/env python3
"""Generate a deterministic SPDX 2.3 JSON SBOM from locked Cargo metadata."""

from __future__ import annotations

import datetime as dt
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import tomllib


ROOT = pathlib.Path(__file__).resolve().parent.parent


def stable_package_key(package: dict[str, object]) -> str:
    """Return an identity that does not embed the checkout's absolute path."""
    source = package.get("source")
    if isinstance(source, str) and source:
        return str(package["id"])

    manifest_path = pathlib.Path(str(package["manifest_path"])).resolve()
    try:
        relative_manifest = manifest_path.relative_to(ROOT.resolve()).as_posix()
    except ValueError:
        # Source-less packages outside the workspace are unusual, but their
        # name/version identity is still preferable to leaking a host path into
        # a supposedly reproducible release document.
        relative_manifest = pathlib.PurePath(str(package["manifest_path"])).name
    return (
        f"path+workspace://{relative_manifest}"
        f"#{package['name']}@{package['version']}"
    )


def spdx_id(package: dict[str, object]) -> str:
    name = str(package["name"])
    version = str(package["version"])
    suffix = hashlib.sha256(stable_package_key(package).encode()).hexdigest()[:12]
    safe = re.sub(r"[^A-Za-z0-9.-]", "-", f"{name}-{version}")
    return f"SPDXRef-Package-{safe}-{suffix}"


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: scripts/generate-spdx.py OUTPUT", file=sys.stderr)
        return 2
    output = pathlib.Path(sys.argv[1])
    output.parent.mkdir(parents=True, exist_ok=True)

    metadata = json.loads(
        subprocess.check_output(
            [
                "cargo",
                "metadata",
                "--format-version",
                "1",
                "--locked",
                "--offline",
            ],
            cwd=ROOT,
            text=True,
        )
    )
    with (ROOT / "Cargo.lock").open("rb") as handle:
        lock = tomllib.load(handle)
    checksums = {
        (package["name"], package["version"], package.get("source")): package.get("checksum")
        for package in lock.get("package", [])
    }

    epoch = int(os.environ.get("SOURCE_DATE_EPOCH", "0"))
    created = dt.datetime.fromtimestamp(epoch, tz=dt.timezone.utc).strftime(
        "%Y-%m-%dT%H:%M:%SZ"
    )
    commit = os.environ.get("SNX_BUILD_COMMIT", "development")
    package_ids = {
        package["id"]: spdx_id(package)
        for package in metadata["packages"]
    }

    packages = []
    for package in sorted(
        metadata["packages"],
        key=lambda item: (item["name"], item["version"], stable_package_key(item)),
    ):
        source = package.get("source")
        download_location = source or "NOASSERTION"
        if download_location.startswith("registry+"):
            download_location = download_location.removeprefix("registry+")
        checksum = checksums.get((package["name"], package["version"], source))
        item = {
            "SPDXID": package_ids[package["id"]],
            "name": package["name"],
            "versionInfo": package["version"],
            "downloadLocation": download_location,
            "filesAnalyzed": False,
            "licenseConcluded": package.get("license") or "NOASSERTION",
            "licenseDeclared": package.get("license") or "NOASSERTION",
            "copyrightText": "NOASSERTION",
            "supplier": "NOASSERTION",
            "externalRefs": [
                {
                    "referenceCategory": "PACKAGE-MANAGER",
                    "referenceType": "purl",
                    "referenceLocator": f"pkg:cargo/{package['name']}@{package['version']}",
                }
            ],
        }
        if checksum and re.fullmatch(r"[0-9a-fA-F]{64}", checksum):
            item["checksums"] = [{"algorithm": "SHA256", "checksumValue": checksum}]
        packages.append(item)

    workspace_ids = set(metadata["workspace_members"])
    relationships = [
        {
            "spdxElementId": "SPDXRef-DOCUMENT",
            "relationshipType": "DESCRIBES",
            "relatedSpdxElement": package_ids[package_id],
        }
        for package_id in sorted(workspace_ids, key=lambda item: package_ids[item])
    ]
    resolve = metadata.get("resolve") or {}
    for node in sorted(
        resolve.get("nodes", []),
        key=lambda item: package_ids.get(item["id"], item["id"]),
    ):
        source_id = package_ids.get(node["id"])
        if not source_id:
            continue
        for dependency in sorted(
            node.get("dependencies", []),
            key=lambda item: package_ids.get(item, item),
        ):
            target_id = package_ids.get(dependency)
            if target_id:
                relationships.append(
                    {
                        "spdxElementId": source_id,
                        "relationshipType": "DEPENDS_ON",
                        "relatedSpdxElement": target_id,
                    }
                )

    namespace_seed = hashlib.sha256(
        f"silent-nexus-1.0.0-{commit}".encode()
    ).hexdigest()
    document = {
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": "Silent-Nexus-1.0.0",
        "documentNamespace": f"https://github.com/silent-protocol/silent-nexus/spdx/{namespace_seed}",
        "creationInfo": {
            "created": created,
            "creators": ["Organization: Silent Protocol", "Tool: generate-spdx.py"],
        },
        "documentDescribes": [
            package_ids[package_id]
            for package_id in sorted(workspace_ids, key=lambda item: package_ids[item])
        ],
        "packages": packages,
        "relationships": relationships,
    }
    output.write_text(
        json.dumps(document, indent=2, sort_keys=True, ensure_ascii=True) + "\n",
        encoding="utf-8",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
