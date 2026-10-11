"""Tests for `audio_output`, the module every audio runtime bundle carries to
hand hedos a finished piece: the formats, the encoding, and the result event
followed by the file in frames. Each bundle holds its own copy, so the copies
are checked to be the same bytes."""

import importlib.util
import io
import json

import pytest
from conftest import RUNTIMES_DIR

BUNDLES = ("python-audio", "python-yue2")


@pytest.fixture(scope="module")
def audio_output():
    path = RUNTIMES_DIR / BUNDLES[0] / "audio_output.py"
    spec = importlib.util.spec_from_file_location("sidecar_audio_output", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_every_audio_bundle_carries_the_same_copy():
    copies = {(RUNTIMES_DIR / bundle / "audio_output.py").read_bytes() for bundle in BUNDLES}
    assert len(copies) == 1


def test_only_flac_and_wav_are_written(audio_output):
    assert audio_output.output_format({}) == "flac"
    assert audio_output.output_format({"format": "WAV"}) == "wav"
    with pytest.raises(audio_output.FormatError, match="mp3"):
        audio_output.output_format({"format": "mp3"})


def test_a_result_is_announced_then_sent_in_frames(audio_output):
    sent = []
    audio_output.send_result(
        lambda kind, payload: sent.append((kind, payload)), b"0123456789", "flac", 48000, 2, 1.5, 4
    )
    kind, payload = sent[0]
    assert kind == 1
    assert json.loads(payload) == {
        "event": "result",
        "format": "flac",
        "bytes": 10,
        "sample_rate": 48000,
        "channels": 2,
        "seconds": 1.5,
    }
    assert sent[1:] == [(2, b"0123"), (2, b"4567"), (2, b"89")]


def test_encoding_writes_a_stereo_flac(audio_output):
    np = pytest.importorskip("numpy")
    soundfile = pytest.importorskip("soundfile")

    frames = np.zeros((4800, 2), dtype=np.float32)
    data = audio_output.encode(frames, 48000, "flac")
    info = soundfile.info(io.BytesIO(data))
    assert (info.samplerate, info.channels, info.frames) == (48000, 2, 4800)
    assert data[:4] == b"fLaC"
