#!/usr/bin/env python3
# file: .github/workflows/scripts/ci_workflow.py
# version: 1.1.0
# guid: f6a7b8c9-d0e1-2f3a-4b5c-6d7e8f9a0b1c

"""CI workflow helper functions for matrix generation and change detection.

This module provides functions to generate optimized test matrices based on
repository configuration and detected file changes, reducing unnecessary CI jobs.
"""

from __future__ import annotations

from dataclasses import dataclass
import json
import subprocess
from typing import Any

import workflow_common


@dataclass
class ChangeDetection:
    """Detected file changes and affected languages.

    Attributes:
        go_changed: True if Go files changed
        python_changed: True if Python files changed
        rust_changed: True if Rust files changed
        node_changed: True if Node.js files changed
        docker_changed: True if Docker files changed
        docs_changed: True if documentation files changed
        workflows_changed: True if GitHub Actions workflows changed
        all_files: List of all changed file paths
    """

    go_changed: bool = False
    python_changed: bool = False
    rust_changed: bool = False
    node_changed: bool = False
    docker_changed: bool = False
    docs_changed: bool = False
    workflows_changed: bool = False
    all_files: list[str] | None = None

    def __post_init__(self) -> None:
        """Initialize all_files as empty list if not provided."""
        if self.all_files is None:
            self.all_files = []


def detect_changes(
    base_ref: str = "origin/main",
    head_ref: str = "HEAD",
) -> ChangeDetection:
    """Detect changed files and determine affected languages.

    Args:
        base_ref: Base git reference for comparison
        head_ref: Head git reference for comparison

    Returns:
        ChangeDetection object with boolean flags for each language/component

    Raises:
        workflow_common.WorkflowError: If git command fails
    """
    try:
        result = subprocess.run(
            ["git", "diff", "--name-only", base_ref, head_ref],
            capture_output=True,
            text=True,
            check=True,
        )
        changed_files = [
            entry.strip()
            for entry in result.stdout.splitlines()
            if entry.strip()
        ]
    except subprocess.CalledProcessError as error:
        raise workflow_common.WorkflowError(
            f"Failed to detect changes: {error}",
            hint="Ensure git repository is initialized and base_ref exists",
        ) from error

    detection = ChangeDetection(all_files=changed_files)

    patterns = {
        "go": [".go", "go.mod", "go.sum"],
        "python": [".py", "requirements.txt", "pyproject.toml", "setup.py"],
        "rust": [".rs", "Cargo.toml", "Cargo.lock"],
        "node": [".js", ".ts", "package.json", "package-lock.json"],
        "docker": ["Dockerfile", ".dockerignore", "docker-compose.yml"],
        "docs": [".md", ".rst", "/docs/"],
        "workflows": [".github/workflows/", ".github/actions/"],
    }

    for file_path in changed_files:
        if any(file_path.endswith(ext) for ext in patterns["go"]):
            detection.go_changed = True
        if any(file_path.endswith(ext) for ext in patterns["python"]):
            detection.python_changed = True
        if any(file_path.endswith(ext) for ext in patterns["rust"]):
            detection.rust_changed = True
        if any(file_path.endswith(ext) for ext in patterns["node"]):
            detection.node_changed = True
        if any(pattern in file_path for pattern in patterns["docker"]):
            detection.docker_changed = True
        if any(pattern in file_path for pattern in patterns["docs"]):
            detection.docs_changed = True
        if any(pattern in file_path for pattern in patterns["workflows"]):
            detection.workflows_changed = True

    return detection


def get_branch_version_target(branch_name: str, language: str) -> str | None:
    """Determine target language version based on branch name.

    Implements parallel release track strategy:
    - main branch: uses latest version from config
    - stable-1-* branches: extract version from branch name
    """
    import re

    if branch_name in {"main", "master", "develop"}:
        return None

    pattern = rf"stable-1-{language}-(.+)$"
    match = re.match(pattern, branch_name)
    if match:
        return match.group(1)
    return None


def is_unresolved_track_marker(version: str) -> bool:
    """True for a `stable-N` placeholder (e.g. `stable-1`) configured as a
    version entry — meaningful ONLY when resolved via a matching
    `stable-N-<language>-<version>` branch name (see
    `get_branch_version_target`). On any other branch there is no concrete
    toolchain/version behind it, so it must never be passed through to a
    real toolchain installer literally — e.g. `rustup toolchain install
    stable-1` is not a valid channel name and always fails.
    """
    import re

    return bool(re.match(r"^stable-\d+$", version))


def generate_test_matrix(
    languages: list[str],
    versions: dict[str, list[str]] | None = None,
    platforms: list[str] | None = None,
    optimize: bool = True,
    branch_name: str | None = None,
) -> dict[str, Any]:
    """Generate optimized test matrix for CI workflows."""
    if branch_name is None:
        try:
            result = subprocess.run(
                ["git", "rev-parse", "--abbrev-ref", "HEAD"],
                capture_output=True,
                text=True,
                check=True,
            )
            branch_name = result.stdout.strip()
        except subprocess.CalledProcessError:
            branch_name = "main"

    if versions is None:
        versions = {
            "go": workflow_common.config_path([], "languages", "versions", "go"),
            "python": workflow_common.config_path(
                [], "languages", "versions", "python"
            ),
            "rust": workflow_common.config_path([], "languages", "versions", "rust"),
            "node": workflow_common.config_path([], "languages", "versions", "node"),
        }

    if platforms is None:
        platforms = workflow_common.config_path(
            ["ubuntu-latest", "macos-latest"],
            "build",
            "platforms",
        )

    matrix_entries: list[dict[str, Any]] = []

    for language in languages:
        lang_versions = versions.get(language, [])
        if not lang_versions:
            print(f"⚠️  No versions configured for {language}, skipping")
            continue

        branch_version = get_branch_version_target(branch_name, language)
        if branch_version:
            print(f"🎯 Branch {branch_name} targets {language} {branch_version}")
            if branch_version not in lang_versions:
                print(
                    f"⚠️  Branch target {branch_version} not in configured versions, "
                    "skipping"
                )
                continue
            for platform in platforms:
                matrix_entries.append(
                    {
                        "language": language,
                        "version": branch_version,
                        "os": platform,
                        "branch": branch_name,
                    }
                )
            continue

        resolved_versions = [
            version for version in lang_versions if not is_unresolved_track_marker(version)
        ]
        if not resolved_versions:
            print(
                f"⚠️  {language}: no concrete version resolved on branch "
                f"{branch_name} (only stable-N track markers configured), skipping"
            )
            continue
        lang_versions = resolved_versions

        if optimize:
            latest_version = lang_versions[-1]
            older_versions = lang_versions[:-1]

            for platform in platforms:
                matrix_entries.append(
                    {
                        "language": language,
                        "version": latest_version,
                        "os": platform,
                        "branch": branch_name,
                    }
                )
            for version in older_versions:
                matrix_entries.append(
                    {
                        "language": language,
                        "version": version,
                        "os": "ubuntu-latest",
                        "branch": branch_name,
                    }
                )
        else:
            for version in lang_versions:
                for platform in platforms:
                    matrix_entries.append(
                        {
                            "language": language,
                            "version": version,
                            "os": platform,
                            "branch": branch_name,
                        }
                    )

    return {"include": matrix_entries}


def should_run_tests(language: str, changes: ChangeDetection) -> bool:
    """Determine whether tests should run for a given language."""
    language_map = {
        "go": changes.go_changed,
        "python": changes.python_changed,
        "rust": changes.rust_changed,
        "node": changes.node_changed,
    }

    if changes.workflows_changed:
        return True

    return language_map.get(language, False)


def get_coverage_threshold(language: str) -> float:
    """Return coverage threshold for the specified language."""
    return workflow_common.config_path(
        80.0,
        "testing",
        "coverage",
        language,
        "threshold",
    )


def format_matrix_summary(matrix: dict[str, Any]) -> str:
    """Format matrix as markdown summary for GitHub Actions."""
    entries = matrix.get("include", [])
    if not entries:
        return "## Test Matrix\n\n❌ No tests to run\n"

    lines = [
        "## Test Matrix",
        "",
        f"**Total Jobs**: {len(entries)}",
        "",
        "| Language | Version | Platform |",
        "|----------|---------|----------|",
    ]

    for entry in entries:
        lang = entry.get("language", "unknown")
        version = entry.get("version", "unknown")
        os_name = entry.get("os", "unknown")
        lines.append(f"| {lang} | {version} | {os_name} |")

    return "\n".join(lines) + "\n"


def main() -> None:
    """Main entry point for CI workflow helper.

    Detects changes, generates test matrix, and outputs results for GitHub
    Actions workflow consumption.
    """
    import argparse

    parser = argparse.ArgumentParser(
        description="Generate CI test matrix",
    )
    parser.add_argument(
        "--base-ref",
        default="origin/main",
        help="Base git reference for change detection",
    )
    parser.add_argument(
        "--optimize",
        action="store_true",
        default=False,
        help="Optimize matrix to reduce job count",
    )
    parser.add_argument(
        "--output-matrix",
        action="store_true",
        help="Output matrix JSON to GITHUB_OUTPUT",
    )

    args = parser.parse_args()

    try:
        with workflow_common.timed_operation("Detect changes"):
            changes = detect_changes(base_ref=args.base_ref)

        print("📊 Change detection results:")
        print(f"   Go: {'✅' if changes.go_changed else '❌'}")
        print(f"   Python: {'✅' if changes.python_changed else '❌'}")
        print(f"   Rust: {'✅' if changes.rust_changed else '❌'}")
        print(f"   Node: {'✅' if changes.node_changed else '❌'}")
        print(f"   Docker: {'✅' if changes.docker_changed else '❌'}")
        print(f"   Docs: {'✅' if changes.docs_changed else '❌'}")
        print(f"   Workflows: {'✅' if changes.workflows_changed else '❌'}")

        languages_to_test: list[str] = []
        for language in ["go", "python", "rust", "node"]:
            if should_run_tests(language, changes):
                languages_to_test.append(language)

        languages_display = ", ".join(languages_to_test) or "None"
        print(f"\n🎯 Languages requiring tests: {languages_display}")

        if languages_to_test:
            with workflow_common.timed_operation("Generate test matrix"):
                matrix = generate_test_matrix(
                    languages_to_test,
                    optimize=args.optimize,
                )

            print(f"\n✅ Generated matrix with {len(matrix['include'])} jobs")

            if args.output_matrix:
                workflow_common.write_output("matrix", json.dumps(matrix))
                workflow_common.write_output("has_tests", "true")

            summary = format_matrix_summary(matrix)
            try:
                workflow_common.append_summary(summary)
            except workflow_common.WorkflowError as error:
                print(workflow_common.sanitize_log(str(error)))
        else:
            print("\n✅ No changes detected, skipping tests")

            if args.output_matrix:
                workflow_common.write_output(
                    "matrix",
                    json.dumps({"include": []}),
                )
                workflow_common.write_output("has_tests", "false")

            try:
                workflow_common.append_summary(
                    "## Test Matrix\n\n✅ No changes detected\n"
                )
            except workflow_common.WorkflowError as error:
                print(workflow_common.sanitize_log(str(error)))

    except Exception as error:  # pylint: disable=broad-except
        workflow_common.handle_error(error, "CI matrix generation")


if __name__ == "__main__":
    main()
