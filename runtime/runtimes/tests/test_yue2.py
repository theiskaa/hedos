"""Tests for the YuE2 sidecar: the request it builds for YuE2, the order and
reporting of YuE2's four stages, and cancel, against a stand-in pipeline that
has YuE2's method signatures but runs nothing."""

import json
from dataclasses import dataclass
from types import SimpleNamespace

import pytest


@dataclass(frozen=True)
class SongRequest:
    """YuE2's own request, with the checks the sidecar relies on it making."""

    style: str
    lyrics: str
    cot: str = "full"
    seed: int = 831001
    abc: str | None = None
    cfg_scale: float | None = None

    def __post_init__(self):
        if self.abc is not None and self.cot == "off":
            raise ValueError("External ABC requires nonempty text and cot=melody/full")
        if self.cfg_scale is not None and not 0 <= self.cfg_scale <= 20:
            raise ValueError("cfg_scale must be finite and in [0,20]")


PROTOCOL = SimpleNamespace(SongRequest=SongRequest)


class FakePipe:
    """YuE2Pipeline's stage methods, recording what they were called with."""

    def __init__(self, tokens=130, cancel_in=None):
        self.generation_config = SimpleNamespace(
            abc=SimpleNamespace(max_tokens=4096), semantic=SimpleNamespace(max_tokens=9000)
        )
        self.tokens = tokens
        self.cancel_in = cancel_in
        self.calls = []

    def run_tokens(self, phase, cancelled, on_token):
        for index in range(self.tokens):
            if cancelled():
                raise InterruptedError(f"Cancelled during {phase}")
            on_token(phase, index)

    def plan(self, *, request, cancelled, on_token):
        self.calls.append(("plan", request))
        self.run_tokens("abc", cancelled, on_token)
        return SimpleNamespace(truncated=False)

    def generate_semantic(self, plan, *, cancelled, on_token):
        self.calls.append(("semantic", plan))
        self.run_tokens("semantic", cancelled, on_token)
        return SimpleNamespace(truncated=True)

    def synthesize(self, semantic, *, cancelled):
        self.calls.append(("synthesize", semantic))
        return "latents"

    def decode(self, latents):
        self.calls.append(("decode", latents))
        return "audio"


@pytest.fixture
def sent(yue2, monkeypatch):
    frames = []
    monkeypatch.setattr(yue2, "send", lambda kind, payload: frames.append((kind, payload)))
    return frames


def events(frames):
    return [json.loads(payload) for kind, payload in frames if kind == 1]


def test_a_song_needs_a_style_and_lyrics(yue2):
    with pytest.raises(yue2.RequestFault, match="style"):
        yue2.song_request({"lyrics": "[verse]\nhi"})
    with pytest.raises(yue2.RequestFault, match="lyrics"):
        yue2.song_request({"prompt": "pop"})
    fields = yue2.song_request({"prompt": " pop ", "lyrics": "[verse]\nhi", "seed": 7})
    assert fields == {"style": "pop", "lyrics": "[verse]\nhi", "seed": 7}


def test_an_instrumental_is_asked_for_in_the_lyrics(yue2):
    fields = yue2.song_request({"prompt": "jazz", "instrumental": True})
    assert fields["lyrics"] == "[instrumental]"
    blank = yue2.song_request({"prompt": "jazz", "instrumental": True, "lyrics": "  "})
    assert blank["lyrics"] == "[instrumental]"


def test_what_yue2_has_no_use_for_is_refused(yue2):
    for key, value in [("duration", 60), ("steps", 30), ("negative_prompt", "noise")]:
        with pytest.raises(yue2.RequestFault, match=key):
            yue2.song_request({"prompt": "pop", "lyrics": "la", key: value})
    with pytest.raises(yue2.RequestFault, match="cot"):
        yue2.song_request({"prompt": "pop", "lyrics": "la", "cot": "some"})


def test_yue2s_own_options_pass_through(yue2):
    fields = yue2.song_request(
        {"prompt": "pop", "lyrics": "la", "cot": "melody", "abc": "X:1", "cfg_scale": 1.5}
    )
    assert (fields["cot"], fields["abc"], fields["cfg_scale"]) == ("melody", "X:1", 1.5)
    assert isinstance(fields["seed"], int), "a seed is drawn when none is given"


def test_companions_are_read_by_role(yue2):
    assert yue2.companions(["vae=/hub/vae", "bad", "codec=/x"]) == {
        "vae": "/hub/vae",
        "codec": "/x",
    }
    assert yue2.companions(None) == {}


def test_the_four_stages_run_in_order_and_say_so(yue2, sent, monkeypatch):
    monkeypatch.setattr(yue2, "pending_op", lambda: None)
    pipe = FakePipe()
    job = yue2.Job()
    fields = yue2.song_request({"prompt": "pop", "lyrics": "la", "seed": 1})
    assert yue2.sing(pipe, fields, job, PROTOCOL) == "audio"
    assert [call[0] for call in pipe.calls] == ["plan", "semantic", "synthesize", "decode"]
    assert pipe.calls[0][1] == SongRequest(style="pop", lyrics="la", seed=1)
    statuses = [event["message"] for event in events(sent) if event["event"] == "status"]
    assert statuses == [
        "Writing the score",
        "Generating audio tokens",
        "The song reached its length limit",
        "Synthesizing",
        "Decoding",
    ]
    steps = [event["n"] for event in events(sent) if event["event"] == "step"]
    # One rising figure over the whole song, in thousandths: the score from 0,
    # its tokens against its 4096 cap, then the audio tokens from 200, and each
    # silent stage marked as it starts.
    assert steps == [0, 3, 6, 200, 203, 207, 750, 950]
    assert steps == sorted(steps), "progress never moves backwards"
    assert all(event["total"] == 1000 for event in events(sent) if event["event"] == "step")


def test_a_cancel_stops_the_token_loop_and_is_remembered(yue2, sent, monkeypatch):
    ops = iter([None] * 10 + ["cancel"])
    monkeypatch.setattr(yue2, "pending_op", lambda: next(ops, None))
    pipe = FakePipe()
    job = yue2.Job()
    with pytest.raises(InterruptedError):
        yue2.sing(pipe, yue2.song_request({"prompt": "p", "lyrics": "l"}), job, PROTOCOL)
    assert [call[0] for call in pipe.calls] == ["plan"]
    assert job.cancelled() and not job.shutdown_requested


def test_a_shutdown_while_singing_ends_the_sidecar(yue2, sent, monkeypatch):
    monkeypatch.setattr(yue2, "pending_op", lambda: "shutdown")
    job = yue2.Job()
    assert job.cancelled() and job.shutdown_requested


def test_yue2s_own_refusals_come_back_as_bad_requests(yue2, sent, monkeypatch):
    monkeypatch.setattr(yue2, "pending_op", lambda: None)
    fields = yue2.song_request({"prompt": "p", "lyrics": "l", "cot": "off", "abc": "X:1"})
    with pytest.raises(yue2.RequestFault, match="cot"):
        yue2.sing(FakePipe(), fields, yue2.Job(), PROTOCOL)
    fields = yue2.song_request({"prompt": "p", "lyrics": "l", "cfg_scale": 99})
    with pytest.raises(yue2.RequestFault, match="cfg_scale"):
        yue2.sing(FakePipe(), fields, yue2.Job(), PROTOCOL)


def test_a_heartbeat_speaks_during_a_silent_stage_and_stops_with_it(yue2, monkeypatch):
    import threading

    beats = []
    heard = threading.Event()

    def record(kind, payload):
        beats.append(json.loads(payload)["message"])
        heard.set()

    monkeypatch.setattr(yue2, "send", record)
    heartbeat = yue2.Heartbeat("Synthesizing", every=0.01)
    with heartbeat:
        assert heard.wait(timeout=5), "no heartbeat within five seconds"
    assert not heartbeat.thread.is_alive(), "leaving the stage stops the heartbeat"
    assert all(beat.startswith("Synthesizing (") for beat in beats)
