"""Shared fixtures for testing the python sidecars' wire protocol and
request-shaping helpers, without loading any model.

Every `runtime/runtimes/*/main.py` redirects fd 1 (stdout) to fd 2 (stderr)
at import time:

    real_stdout = os.dup(1)
    os.dup2(2, 1)

so that anything a model library prints on stdout can't corrupt the framed
protocol, which is written straight to the saved `real_stdout` fd instead.
That's a process-wide side effect: it mutates fd 1 for the whole test
process, not just for the importing module. Importing a second sidecar
after the first would have its own `os.dup(1)` capture the
already-redirected fd 1 (i.e. stderr), not the original stdout. Tests must
never rely on a module's own `real_stdout` value for anything other than
"some fd we monkeypatch before calling send()"; the `_preserve_fd1` fixture
below restores fd 1 to what it was before this test session started, so the
redirect doesn't leak into pytest's own output reporting.
"""

import importlib.util
import os
import sys
from pathlib import Path

import pytest

RUNTIMES_DIR = Path(__file__).resolve().parent.parent


def load_sidecar_module(dirname, module_name):
    """Import a sidecar's main.py as an isolated module object.

    Uses spec_from_file_location because every sidecar lives in a hyphenated
    directory (e.g. `python-mlx-lm`) with no `__init__.py`, so it isn't a
    valid dotted package name a plain `import` could reach.
    """
    path = RUNTIMES_DIR / dirname / "main.py"
    spec = importlib.util.spec_from_file_location(module_name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture(scope="session", autouse=True)
def _preserve_fd1():
    saved = os.dup(1)
    try:
        yield
    finally:
        os.dup2(saved, 1)
        os.close(saved)


@pytest.fixture(scope="session")
def mlx_lm():
    """python-mlx-lm/main.py — the representative sidecar for the frame
    protocol; every sidecar copies this same send/read_exact/read_frame/
    read_request implementation verbatim."""
    return load_sidecar_module("python-mlx-lm", "hedos_sidecar_mlx_lm")


@pytest.fixture(scope="session")
def embeddings():
    return load_sidecar_module("python-embeddings", "hedos_sidecar_embeddings")


@pytest.fixture(scope="session")
def mlx_vlm():
    return load_sidecar_module("python-mlx-vlm", "hedos_sidecar_mlx_vlm")


@pytest.fixture
def stdin_pipe(mlx_lm):
    """Redirect fd 0 (what read_exact/read_frame hardcode as stdin) to a
    pipe the test controls, restoring the real fd 0 afterward."""
    saved_fd0 = os.dup(0)
    read_end, write_end = os.pipe()
    os.dup2(read_end, 0)
    os.close(read_end)
    try:
        yield write_end
    finally:
        os.dup2(saved_fd0, 0)
        os.close(saved_fd0)
        try:
            os.close(write_end)
        except OSError:
            pass


@pytest.fixture
def stdout_pipe(mlx_lm, monkeypatch):
    """Redirect the module's `real_stdout` handle (what `send` writes to)
    to a pipe the test controls."""
    read_end, write_end = os.pipe()
    monkeypatch.setattr(mlx_lm, "real_stdout", write_end)
    try:
        yield read_end
    finally:
        os.close(read_end)
        os.close(write_end)
