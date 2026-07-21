"""Tests for the sidecar wire protocol: a 4-byte little-endian length
prefix (covering the type byte plus the payload) followed by a 1-byte frame
type and the payload body.

Exercised against `python-mlx-lm/main.py` as the representative sidecar —
every sidecar copies this exact protocol implementation. `fake_sidecar.py`,
the test double every Rust integration test drives instead of a real
`main.py`, implements the identical framing; a drift-guard test below pins
that its `send()` source stays byte-for-byte the same construction.

`read_exact`/`read_frame` hardcode `os.read(0, ...)` (real stdin) and `send`
writes through the module's `real_stdout` handle, so these are exercised
for real — not reimplemented — using `os.pipe()` to stand in for stdin and
for the real stdout fd.
"""

import os
import struct
from pathlib import Path


def test_send_frame_layout_is_length_prefix_plus_type_byte_plus_payload(mlx_lm, stdout_pipe):
    mlx_lm.send(3, b"hello")
    raw = os.read(stdout_pipe, 4096)
    (length,) = struct.unpack("<I", raw[:4])
    assert length == len(b"hello") + 1
    assert raw[4] == 3
    assert raw[5:] == b"hello"


def test_send_read_frame_round_trip(mlx_lm, stdin_pipe, stdout_pipe):
    mlx_lm.send(7, b"payload-bytes")
    frame = os.read(stdout_pipe, 4096)
    os.write(stdin_pipe, frame)
    frame_type, body = mlx_lm.read_frame()
    assert frame_type == 7
    assert body == b"payload-bytes"


def test_send_json_round_trips_through_read_request(mlx_lm, stdin_pipe, stdout_pipe):
    mlx_lm.send_json({"event": "ready"})
    frame = os.read(stdout_pipe, 4096)
    os.write(stdin_pipe, frame)
    assert mlx_lm.read_request() == {"event": "ready"}


def test_read_exact_returns_none_on_immediately_closed_stream(mlx_lm, stdin_pipe):
    os.close(stdin_pipe)
    assert mlx_lm.read_exact(4) is None


def test_read_exact_returns_none_on_short_read_then_close(mlx_lm, stdin_pipe):
    os.write(stdin_pipe, b"ab")
    os.close(stdin_pipe)
    assert mlx_lm.read_exact(4) is None


def test_read_frame_returns_none_when_header_is_truncated(mlx_lm, stdin_pipe):
    os.write(stdin_pipe, b"\x01\x00")
    os.close(stdin_pipe)
    assert mlx_lm.read_frame() is None


def test_read_frame_returns_none_when_body_is_truncated(mlx_lm, stdin_pipe):
    # Header claims a 10-byte body but only 2 bytes follow before EOF.
    os.write(stdin_pipe, struct.pack("<I", 10) + b"ab")
    os.close(stdin_pipe)
    assert mlx_lm.read_frame() is None


def test_read_request_returns_empty_dict_on_invalid_json(mlx_lm, stdin_pipe, stdout_pipe):
    mlx_lm.send(1, b"not-json{")
    frame = os.read(stdout_pipe, 4096)
    os.write(stdin_pipe, frame)
    assert mlx_lm.read_request() == {}


def test_read_request_returns_none_on_closed_stream(mlx_lm, stdin_pipe):
    os.close(stdin_pipe)
    assert mlx_lm.read_request() is None


def test_send_matches_fake_sidecar_reference_construction():
    """Drift guard: fake_sidecar.py's send() must stay this exact
    construction — struct.pack("<I", len(payload)+1) + bytes([frame_type])
    + payload — since it's what every Rust integration test asserts
    against in place of a real main.py."""
    support_dir = Path(__file__).resolve().parents[2] / "tests" / "support"
    text = (support_dir / "fake_sidecar.py").read_text()
    assert 'struct.pack("<I", len(payload) + 1) + bytes([frame_type]) + payload' in text
