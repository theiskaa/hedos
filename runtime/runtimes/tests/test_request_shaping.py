"""Tests for the pure, per-sidecar request-shaping helpers — the functions
that turn a JSON request dict into the kwargs a model call needs. None of
these load a model or touch the network; they operate on plain dicts,
strings, and (for materialize_images) a scratch directory.
"""

import base64
from pathlib import Path


def test_sampler_kwargs_maps_and_coerces_types(mlx_lm):
    result = mlx_lm.sampler_kwargs({"temperature": 1, "top_k": 5})
    assert result == {"temp": 1.0, "top_k": 5}
    assert isinstance(result["temp"], float)
    assert isinstance(result["top_k"], int)


def test_sampler_kwargs_all_recognized_keys(mlx_lm):
    request = {"temperature": 0.7, "top_p": 0.95, "min_p": 0.05, "top_k": 40}
    assert mlx_lm.sampler_kwargs(request) == {
        "temp": 0.7,
        "top_p": 0.95,
        "min_p": 0.05,
        "top_k": 40,
    }


def test_sampler_kwargs_ignores_unknown_keys(mlx_lm):
    assert mlx_lm.sampler_kwargs({"unknown": "value", "seed": 1}) == {}


def test_sampler_kwargs_omits_missing_keys(mlx_lm):
    result = mlx_lm.sampler_kwargs({"top_p": 0.9})
    assert result == {"top_p": 0.9}
    assert "temp" not in result
    assert "min_p" not in result
    assert "top_k" not in result


def test_sampler_kwargs_empty_request_yields_empty_kwargs(mlx_lm):
    assert mlx_lm.sampler_kwargs({}) == {}


def test_stop_strings_absent_returns_empty_list(mlx_lm):
    assert mlx_lm.stop_strings({}) == []


def test_stop_strings_single_string_is_wrapped_in_a_list(mlx_lm):
    assert mlx_lm.stop_strings({"stop": "END"}) == ["END"]


def test_stop_strings_filters_non_string_and_empty_entries(mlx_lm):
    assert mlx_lm.stop_strings({"stop": ["END", "", 5, None, "STOP"]}) == ["END", "STOP"]


def test_as_list_none_returns_empty(embeddings):
    assert embeddings.as_list(None) == []


def test_as_list_wraps_a_single_string(embeddings):
    assert embeddings.as_list("hello") == ["hello"]


def test_as_list_passes_through_an_iterable(embeddings):
    assert embeddings.as_list(["a", "b"]) == ["a", "b"]


def test_materialize_images_writes_files_and_strips_the_images_key(mlx_vlm, tmp_path):
    encoded = base64.b64encode(b"fake-image-bytes").decode()
    messages = [{"role": "user", "content": "hi", "images": [encoded]}]

    stripped, paths = mlx_vlm.materialize_images(messages, str(tmp_path))

    assert stripped == [{"role": "user", "content": "hi"}]
    assert len(paths) == 1
    assert Path(paths[0]).read_bytes() == b"fake-image-bytes"


def test_materialize_images_handles_messages_with_no_images(mlx_vlm, tmp_path):
    messages = [{"role": "user", "content": "hi"}]

    stripped, paths = mlx_vlm.materialize_images(messages, str(tmp_path))

    assert stripped == messages
    assert paths == []
