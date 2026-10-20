"""Isolate docs tests from host discovery overrides such as ``$BOB_DOCS_*_DIR``."""

from __future__ import annotations

import os

import pytest


@pytest.fixture(autouse=True)
def _clear_docs_discovery_env(monkeypatch: pytest.MonkeyPatch) -> None:
    for key in list(os.environ):
        if key.startswith("BOB_DOCS_") and key.endswith("_DIR"):
            monkeypatch.delenv(key, raising=False)
