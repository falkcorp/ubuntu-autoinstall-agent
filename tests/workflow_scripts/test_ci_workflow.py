#!/usr/bin/env python3
# file: tests/workflow_scripts/test_ci_workflow.py
# version: 1.1.0
# guid: c3d4e5f6-a7b8-9c0d-1e2f-3a4b5c6d7e8f

"""Unit tests for ci_workflow module."""

from __future__ import annotations

from pathlib import Path
import sys
from unittest.mock import MagicMock

import pytest

SCRIPTS_DIR = Path(__file__).resolve().parents[2] / ".github/workflows/scripts"
sys.path.insert(0, str(SCRIPTS_DIR))

import ci_workflow  # pylint: disable=wrong-import-position
import workflow_common  # pylint: disable=wrong-import-position


@pytest.fixture(autouse=True)
def reset_config_cache() -> None:
    """Reset global config cache between tests."""
    workflow_common._CONFIG_CACHE = None  # type: ignore[attr-defined]
    yield
    workflow_common._CONFIG_CACHE = None  # type: ignore[attr-defined]


def test_change_detection_dataclass_defaults() -> None:
    """ChangeDetection initializes with expected default values."""
    detection = ci_workflow.ChangeDetection()

    assert detection.go_changed is False
    assert detection.python_changed is False
    assert detection.all_files == []


def test_detect_changes_go_files(monkeypatch: pytest.MonkeyPatch) -> None:
    """detect_changes marks Go files correctly."""
    mock_result = MagicMock()
    mock_result.stdout = "main.go\ngo.mod\n"
    monkeypatch.setattr(
        ci_workflow.subprocess,
        "run",
        MagicMock(return_value=mock_result),
    )

    changes = ci_workflow.detect_changes()

    assert changes.go_changed is True
    assert changes.python_changed is False
    assert "main.go" in changes.all_files


def test_detect_changes_python_files(monkeypatch: pytest.MonkeyPatch) -> None:
    """detect_changes marks Python files correctly."""
    mock_result = MagicMock()
    mock_result.stdout = "script.py\nrequirements.txt\n"
    monkeypatch.setattr(
        ci_workflow.subprocess,
        "run",
        MagicMock(return_value=mock_result),
    )

    changes = ci_workflow.detect_changes()

    assert changes.python_changed is True
    assert changes.go_changed is False


def test_detect_changes_workflow_files(monkeypatch: pytest.MonkeyPatch) -> None:
    """detect_changes marks workflow file updates."""
    mock_result = MagicMock()
    mock_result.stdout = ".github/workflows/ci.yml\n"
    monkeypatch.setattr(
        ci_workflow.subprocess,
        "run",
        MagicMock(return_value=mock_result),
    )

    changes = ci_workflow.detect_changes()

    assert changes.workflows_changed is True


def test_detect_changes_git_failure(monkeypatch: pytest.MonkeyPatch) -> None:
    """detect_changes raises WorkflowError when git diff fails."""
    import subprocess as sp  # Local alias to avoid confusion

    monkeypatch.setattr(
        ci_workflow.subprocess,
        "run",
        MagicMock(side_effect=sp.CalledProcessError(1, "git")),
    )

    with pytest.raises(workflow_common.WorkflowError) as exc_info:
        ci_workflow.detect_changes()

    assert "Failed to detect changes" in str(exc_info.value)


def test_generate_test_matrix_optimized() -> None:
    """generate_test_matrix optimizes matrix for latest version."""
    languages = ["go"]
    versions = {"go": ["1.23", "1.24"]}
    platforms = ["ubuntu-latest", "macos-latest"]

    matrix = ci_workflow.generate_test_matrix(
        languages,
        versions,
        platforms,
        optimize=True,
        branch_name="main",
    )

    entries = matrix["include"]
    assert len(entries) == 3

    latest_entries = [entry for entry in entries if entry["version"] == "1.24"]
    assert len(latest_entries) == 2
    assert {entry["os"] for entry in latest_entries} == {
        "ubuntu-latest",
        "macos-latest",
    }

    older_entries = [entry for entry in entries if entry["version"] == "1.23"]
    assert len(older_entries) == 1
    assert older_entries[0]["os"] == "ubuntu-latest"


def test_generate_test_matrix_full() -> None:
    """generate_test_matrix returns full matrix when not optimized."""
    languages = ["go"]
    versions = {"go": ["1.23", "1.24"]}
    platforms = ["ubuntu-latest", "macos-latest"]

    matrix = ci_workflow.generate_test_matrix(
        languages,
        versions,
        platforms,
        optimize=False,
        branch_name="main",
    )

    assert len(matrix["include"]) == 4


def test_generate_test_matrix_multiple_languages() -> None:
    """generate_test_matrix supports multiple languages."""
    languages = ["go", "python"]
    versions = {"go": ["1.24"], "python": ["3.13"]}
    platforms = ["ubuntu-latest"]

    matrix = ci_workflow.generate_test_matrix(
        languages,
        versions,
        platforms,
        optimize=False,
        branch_name="main",
    )

    entries = matrix["include"]
    assert len(entries) == 2
    assert {entry["language"] for entry in entries} == {"go", "python"}


def test_generate_test_matrix_no_versions(
    capsys: pytest.CaptureFixture[str],
) -> None:
    """generate_test_matrix skips languages with no configured versions."""
    languages = ["go", "unknown"]
    versions = {"go": ["1.24"]}
    platforms = ["ubuntu-latest"]

    matrix = ci_workflow.generate_test_matrix(
        languages,
        versions,
        platforms,
        branch_name="main",
    )

    captured = capsys.readouterr()
    assert "No versions configured for unknown" in captured.out
    assert len(matrix["include"]) == 1


def test_should_run_tests_language_changed() -> None:
    """should_run_tests returns True when language changed."""
    changes = ci_workflow.ChangeDetection(go_changed=True)

    assert ci_workflow.should_run_tests("go", changes) is True


def test_should_run_tests_workflows_changed() -> None:
    """should_run_tests returns True when workflow files changed."""
    changes = ci_workflow.ChangeDetection(workflows_changed=True)

    assert ci_workflow.should_run_tests("go", changes) is True


def test_should_run_tests_no_changes() -> None:
    """should_run_tests returns False when language unchanged."""
    changes = ci_workflow.ChangeDetection(python_changed=True)

    assert ci_workflow.should_run_tests("go", changes) is False


def test_get_coverage_threshold_from_config(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """get_coverage_threshold reads values from config file."""
    config_file = tmp_path / ".github" / "repository-config.yml"
    config_file.parent.mkdir(parents=True)
    config_file.write_text(
        "testing:\n  coverage:\n    python:\n      threshold: 85.5\n",
        encoding="utf-8",
    )
    monkeypatch.chdir(tmp_path)

    threshold = ci_workflow.get_coverage_threshold("python")

    assert threshold == 85.5


def test_get_coverage_threshold_default(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """get_coverage_threshold falls back to default value."""
    config_file = tmp_path / ".github" / "repository-config.yml"
    config_file.parent.mkdir(parents=True)
    config_file.write_text("empty: {}\n", encoding="utf-8")
    monkeypatch.chdir(tmp_path)

    threshold = ci_workflow.get_coverage_threshold("python")

    assert threshold == 80.0


def test_format_matrix_summary_with_entries() -> None:
    """format_matrix_summary returns markdown table."""
    matrix = {
        "include": [
            {"language": "go", "version": "1.24", "os": "ubuntu-latest"},
            {"language": "python", "version": "3.13", "os": "macos-latest"},
        ]
    }

    summary = ci_workflow.format_matrix_summary(matrix)

    assert "## Test Matrix" in summary
    assert "**Total Jobs**: 2" in summary
    assert "| go | 1.24 | ubuntu-latest |" in summary
    assert "| python | 3.13 | macos-latest |" in summary


def test_format_matrix_summary_empty() -> None:
    """format_matrix_summary handles empty matrix."""
    summary = ci_workflow.format_matrix_summary({"include": []})

    assert "❌ No tests to run" in summary


def test_get_branch_version_target_main_branch() -> None:
    """get_branch_version_target returns None for main branch."""
    assert ci_workflow.get_branch_version_target("main", "go") is None


def test_get_branch_version_target_stable_branch() -> None:
    """get_branch_version_target extracts version for stable branch."""
    result = ci_workflow.get_branch_version_target("stable-1-go-1.23", "go")

    assert result == "1.23"


def test_get_branch_version_target_different_language() -> None:
    """get_branch_version_target returns None for other languages."""
    result = ci_workflow.get_branch_version_target("stable-1-go-1.23", "python")

    assert result is None


def test_get_branch_version_target_python_stable() -> None:
    """get_branch_version_target handles Python stable branches."""
    result = ci_workflow.get_branch_version_target(
        "stable-1-python-3.13",
        "python",
    )

    assert result == "3.13"


def test_generate_test_matrix_stable_branch() -> None:
    """generate_test_matrix locks versions on stable branch."""
    languages = ["go"]
    versions = {"go": ["1.23", "1.24", "1.25"]}
    platforms = ["ubuntu-latest", "macos-latest"]

    matrix = ci_workflow.generate_test_matrix(
        languages,
        versions,
        platforms,
        optimize=True,
        branch_name="stable-1-go-1.23",
    )

    entries = matrix["include"]
    assert len(entries) == 2
    assert all(entry["version"] == "1.23" for entry in entries)
    assert {entry["os"] for entry in entries} == {
        "ubuntu-latest",
        "macos-latest",
    }


def test_generate_test_matrix_main_branch_uses_latest() -> None:
    """generate_test_matrix favors latest versions on main branch."""
    languages = ["go"]
    versions = {"go": ["1.23", "1.24", "1.25"]}
    platforms = ["ubuntu-latest"]

    matrix = ci_workflow.generate_test_matrix(
        languages,
        versions,
        platforms,
        optimize=True,
        branch_name="main",
    )

    entries = matrix["include"]
    assert any(entry["version"] == "1.25" for entry in entries)
    assert any(entry["version"] == "1.23" for entry in entries)
    assert any(entry["version"] == "1.24" for entry in entries)


def test_generate_test_matrix_invalid_branch_version(
    capsys: pytest.CaptureFixture[str],
) -> None:
    """generate_test_matrix warns when branch version missing."""
    languages = ["go"]
    versions = {"go": ["1.24", "1.25"]}
    platforms = ["ubuntu-latest"]

    matrix = ci_workflow.generate_test_matrix(
        languages,
        versions,
        platforms,
        branch_name="stable-1-go-1.23",
    )

    captured = capsys.readouterr()
    assert "Branch target 1.23 not in configured versions" in captured.out
    assert len(matrix["include"]) == 0


def test_is_unresolved_track_marker() -> None:
    """is_unresolved_track_marker matches only `stable-N`-shaped strings."""
    assert ci_workflow.is_unresolved_track_marker("stable-1") is True
    assert ci_workflow.is_unresolved_track_marker("stable-2") is True
    assert ci_workflow.is_unresolved_track_marker("stable") is False
    assert ci_workflow.is_unresolved_track_marker("1.23") is False
    assert ci_workflow.is_unresolved_track_marker("stable-1.23") is False


def test_generate_test_matrix_skips_unresolved_track_marker() -> None:
    """A `stable-N` config entry never reaches the matrix as a literal
    version on a branch that doesn't resolve it — regression test for
    `rustup toolchain install stable-1` failing in CI because the "rust"
    version list (`["stable", "stable-1"]`) was passed straight through
    unfiltered on ordinary (non `stable-1-rust-*`) branches.
    """
    languages = ["rust"]
    versions = {"rust": ["stable", "stable-1"]}
    platforms = ["ubuntu-latest", "macos-latest"]

    matrix = ci_workflow.generate_test_matrix(
        languages,
        versions,
        platforms,
        optimize=True,
        branch_name="feat/some-feature",
    )

    entries = matrix["include"]
    assert entries, "the resolvable 'stable' entry must still produce jobs"
    assert all(entry["version"] != "stable-1" for entry in entries)
    assert all(entry["version"] == "stable" for entry in entries)


def test_generate_test_matrix_only_track_markers_configured(
    capsys: pytest.CaptureFixture[str],
) -> None:
    """If a language's ENTIRE version list is unresolved track markers,
    generate_test_matrix skips it (with a warning) rather than emitting
    invalid matrix entries."""
    languages = ["rust"]
    versions = {"rust": ["stable-1", "stable-2"]}
    platforms = ["ubuntu-latest"]

    matrix = ci_workflow.generate_test_matrix(
        languages,
        versions,
        platforms,
        branch_name="main",
    )

    captured = capsys.readouterr()
    assert "no concrete version resolved" in captured.out
    assert len(matrix["include"]) == 0
