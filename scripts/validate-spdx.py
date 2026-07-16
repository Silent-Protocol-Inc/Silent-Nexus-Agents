#!/usr/bin/env python3
"""Perform the release-required structural validation of an SPDX JSON SBOM."""

from __future__ import annotations

import json
import pathlib
import re
import sys


def fail(message: str) -> int:
    print(f"error: {message}", file=sys.stderr)
    return 1


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: scripts/validate-spdx.py SBOM", file=sys.stderr)
        return 2
    path = pathlib.Path(sys.argv[1])
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        return fail(f"cannot parse SPDX JSON: {error}")

    if document.get("spdxVersion") != "SPDX-2.3":
        return fail("spdxVersion must be SPDX-2.3")
    if document.get("dataLicense") != "CC0-1.0":
        return fail("dataLicense must be CC0-1.0")
    if document.get("SPDXID") != "SPDXRef-DOCUMENT":
        return fail("document SPDXID is missing")
    if not document.get("documentNamespace", "").startswith("https://"):
        return fail("documentNamespace must be an HTTPS URI")
    if not document.get("creationInfo", {}).get("created"):
        return fail("creationInfo.created is missing")

    packages = document.get("packages")
    if not isinstance(packages, list) or not packages:
        return fail("packages must be a non-empty list")
    identifiers = set()
    for package in packages:
        identifier = package.get("SPDXID", "")
        if not re.fullmatch(r"SPDXRef-[A-Za-z0-9.-]+", identifier):
            return fail(f"invalid package SPDXID: {identifier!r}")
        if identifier in identifiers:
            return fail(f"duplicate package SPDXID: {identifier}")
        identifiers.add(identifier)
        for field in (
            "name",
            "versionInfo",
            "downloadLocation",
            "licenseConcluded",
            "licenseDeclared",
            "copyrightText",
        ):
            if field not in package:
                return fail(f"{identifier} is missing {field}")

    described = set(document.get("documentDescribes", []))
    if not described or not described.issubset(identifiers):
        return fail("documentDescribes must reference packaged SPDX identifiers")
    print(f"SPDX validation: {len(packages)} packages, {len(described)} workspace crates")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
