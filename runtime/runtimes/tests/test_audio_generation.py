"""Tests for the audio-generation sidecar's pure helpers: which family drives
a model folder, how the job contract maps onto a pipeline's own keyword
arguments, and how MusicGen's steps are counted. None of these load a
model."""

import json

import pytest

STABLE_AUDIO = {
    "prompt": None,
    "audio_end_in_s": None,
    "audio_start_in_s": 0.0,
    "num_inference_steps": 100,
    "guidance_scale": 7.0,
    "negative_prompt": None,
    "generator": None,
    "output_type": "pt",
    "callback": None,
    "callback_steps": 1,
}

ACE_STEP = {
    "prompt": None,
    "lyrics": None,
    "audio_duration": 60.0,
    "num_inference_steps": 8,
    "guidance_scale": None,
    "generator": None,
    "callback_on_step_end": None,
}


def test_family_reads_the_pipeline_class_or_the_model_type(audio_gen, tmp_path):
    pipeline = tmp_path / "pipeline"
    pipeline.mkdir()
    (pipeline / "model_index.json").write_text(json.dumps({"_class_name": "AceStepPipeline"}))
    assert audio_gen.family_of(str(pipeline)) == ("diffusers", "AceStepPipeline")

    musicgen = tmp_path / "musicgen"
    musicgen.mkdir()
    (musicgen / "config.json").write_text(json.dumps({"model_type": "musicgen"}))
    assert audio_gen.family_of(str(musicgen)) == ("musicgen", "musicgen")

    chat = tmp_path / "chat"
    chat.mkdir()
    (chat / "config.json").write_text(json.dumps({"model_type": "llama"}))
    assert audio_gen.family_of(str(chat)) is None


def test_the_contract_maps_onto_each_pipelines_own_names(audio_gen):
    request = {"prompt": "rain", "duration": 12, "steps": "30", "guidance": 6, "seed": 1}
    assert audio_gen.pipeline_options(STABLE_AUDIO, request) == {
        "prompt": "rain",
        "audio_end_in_s": 12.0,
        "num_inference_steps": 30,
        "guidance_scale": 6.0,
    }
    song = {"prompt": "pop", "lyrics": "[verse]\nhi", "duration": 90}
    assert audio_gen.pipeline_options(ACE_STEP, song) == {
        "prompt": "pop",
        "lyrics": "[verse]\nhi",
        "audio_duration": 90.0,
    }


def test_lyrics_are_refused_by_a_model_that_cannot_sing(audio_gen):
    with pytest.raises(audio_gen.RequestFault, match="lyrics"):
        audio_gen.pipeline_options(STABLE_AUDIO, {"prompt": "x", "lyrics": "la la"})


def test_instrumental_asks_a_singer_for_no_vocals_and_is_free_elsewhere(audio_gen):
    request = {"prompt": "x", "instrumental": True}
    assert audio_gen.pipeline_options(ACE_STEP, request)["lyrics"] == "[instrumental]"
    blank = {"prompt": "x", "instrumental": True, "lyrics": "  "}
    assert audio_gen.pipeline_options(ACE_STEP, blank)["lyrics"] == "[instrumental]"
    assert "lyrics" not in audio_gen.pipeline_options(STABLE_AUDIO, request)


def test_blank_values_are_left_to_the_pipeline(audio_gen):
    request = {"prompt": "x", "negative_prompt": "  ", "lyrics": ""}
    assert audio_gen.pipeline_options(STABLE_AUDIO, request) == {"prompt": "x"}


def test_a_key_the_pipeline_has_no_name_for_is_refused(audio_gen):
    with pytest.raises(audio_gen.RequestFault, match="negative_prompt"):
        audio_gen.pipeline_options(ACE_STEP, {"prompt": "x", "negative_prompt": "noise"})


def test_musicgen_writes_fifty_tokens_a_second_and_takes_no_lyrics(audio_gen):
    assert audio_gen.musicgen_tokens(10, 50) == 500
    assert audio_gen.musicgen_tokens(0.001, 50) == 1
    with pytest.raises(audio_gen.RequestFault, match="lyrics"):
        audio_gen.check_musicgen_request({"lyrics": "la"})
    audio_gen.check_musicgen_request({"prompt": "x", "duration": 5, "guidance": 3})


class _Ids:
    shape = (4, 7)
    device = "cpu"


class _Torch:
    bool = "bool"

    @staticmethod
    def zeros(count, dtype=None, device=None):
        return [False] * count


class _Criterion:
    """transformers' StoppingCriteria base, which holds no state of its own."""


def test_musicgen_steps_are_counted_by_a_criterion_that_never_stops(audio_gen, monkeypatch):
    ticks = []
    tracker = audio_gen.Steps()
    tracker.total = 10
    monkeypatch.setattr(tracker, "tick", ticks.append)
    counter = audio_gen.step_counter(tracker, 3, _Torch, _Criterion)
    results = [counter(_Ids(), None) for _ in range(12)]
    assert ticks == [3, 6, 9, 10]
    assert all(result == [False] * 4 for result in results)


def test_a_cancel_noticed_between_musicgen_steps_stops_it(audio_gen, monkeypatch):
    monkeypatch.setattr(audio_gen, "send", lambda kind, payload: None)
    monkeypatch.setattr(audio_gen, "pending_op", lambda: "cancel")
    tracker = audio_gen.Steps()
    tracker.total = 100
    counter = audio_gen.step_counter(tracker, 1, _Torch, _Criterion)
    with pytest.raises(audio_gen.CancelJob):
        counter(_Ids(), None)
    assert not tracker.shutdown_requested


def test_the_sample_rate_is_read_where_each_pipeline_keeps_it(audio_gen):
    from types import SimpleNamespace as Ns

    ace = Ns(sample_rate=48000, vae=Ns(config=Ns()))
    stable = Ns(vae=Ns(sampling_rate=44100))
    ldm = Ns(vae=Ns(config=Ns()), vocoder=Ns(config=Ns(sampling_rate=16000)))
    assert audio_gen.sample_rate_of(ace) == 48000
    assert audio_gen.sample_rate_of(stable) == 44100
    assert audio_gen.sample_rate_of(ldm) == 16000
