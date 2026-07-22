"""Tests for the mlx-lm sidecar's end-of-turn tool-call extraction — the pure
parsers that turn a model's finished text into structured calls plus leftover
plain text. Each model family emits its own format; unrecognized output must
pass through as text unchanged, never fail.
"""

import json


def test_plain_text_passes_through_unchanged(mlx_lm):
    text = "The answer is 42."
    assert mlx_lm.extract_tool_calls(text) == (text, [])


def test_call_from_value_requires_a_named_dict(mlx_lm):
    assert mlx_lm.call_from_value("nope") is None
    assert mlx_lm.call_from_value({"arguments": {}}) is None
    assert mlx_lm.call_from_value({"name": ""}) is None
    assert mlx_lm.call_from_value({"name": 5}) is None


def test_call_from_value_rejects_non_object_arguments(mlx_lm):
    assert mlx_lm.call_from_value({"name": "f", "arguments": "text"}) is None
    assert mlx_lm.call_from_value({"name": "f", "arguments": [1]}) is None


def test_call_from_value_defaults_arguments_and_accepts_parameters(mlx_lm):
    assert mlx_lm.call_from_value({"name": "f"}) == {"name": "f", "arguments": {}}
    assert mlx_lm.call_from_value({"name": "f", "parameters": {"a": 1}}) == {
        "name": "f",
        "arguments": {"a": 1},
    }


def test_call_from_value_keeps_a_provided_id(mlx_lm):
    call = mlx_lm.call_from_value({"name": "f", "arguments": {}, "id": "abc123def"})
    assert call == {"name": "f", "arguments": {}, "id": "abc123def"}


def test_tagged_block_with_surrounding_text(mlx_lm):
    text = 'I will read it.\n<tool_call>{"name": "read", "arguments": {"path": "a"}}</tool_call>'
    remaining, calls = mlx_lm.extract_tool_calls(text)
    assert remaining == "I will read it.\n"
    assert calls == [{"name": "read", "arguments": {"path": "a"}}]


def test_multiple_tagged_blocks_yield_multiple_calls(mlx_lm):
    text = (
        '<tool_call>{"name": "a", "arguments": {}}</tool_call>'
        '<tool_call>{"name": "b", "arguments": {}}</tool_call>'
    )
    remaining, calls = mlx_lm.extract_tool_calls(text)
    assert remaining == ""
    assert [call["name"] for call in calls] == ["a", "b"]


def test_malformed_tagged_block_stays_text(mlx_lm):
    text = "<tool_call>not json</tool_call>"
    assert mlx_lm.extract_tool_calls(text) == (text, [])


def test_unclosed_tagged_block_stays_text(mlx_lm):
    text = '<tool_call>{"name": "read"'
    assert mlx_lm.extract_tool_calls(text) == (text, [])


def test_mistral_marker_with_an_array(mlx_lm):
    text = '[TOOL_CALLS][{"name": "read", "arguments": {"path": "a"}, "id": "abc123def"}]'
    remaining, calls = mlx_lm.extract_tool_calls(text)
    assert remaining == ""
    assert calls == [{"name": "read", "arguments": {"path": "a"}, "id": "abc123def"}]


def test_mistral_marker_with_invalid_json_stays_text(mlx_lm):
    text = "[TOOL_CALLS]nonsense"
    assert mlx_lm.extract_tool_calls(text) == (text, [])


def test_python_tag_with_json_becomes_a_call(mlx_lm):
    text = '<|python_tag|>{"name": "read", "parameters": {"path": "a"}}'
    remaining, calls = mlx_lm.extract_tool_calls(text)
    assert remaining == ""
    assert calls == [{"name": "read", "arguments": {"path": "a"}}]


def test_python_tag_with_code_stays_text(mlx_lm):
    text = '<|python_tag|>wolfram_alpha.query("solve x^2")'
    assert mlx_lm.extract_tool_calls(text) == (text, [])


def test_bare_json_object_becomes_a_call(mlx_lm):
    text = '  {"name": "read", "arguments": {"path": "a"}}\n'
    remaining, calls = mlx_lm.extract_tool_calls(text)
    assert remaining == ""
    assert calls == [{"name": "read", "arguments": {"path": "a"}}]


def test_bare_json_array_becomes_calls_only_when_every_entry_parses(mlx_lm):
    good = json.dumps([{"name": "a", "arguments": {}}, {"name": "b", "arguments": {}}])
    remaining, calls = mlx_lm.extract_tool_calls(good)
    assert remaining == ""
    assert [call["name"] for call in calls] == ["a", "b"]

    mixed = json.dumps([{"name": "a", "arguments": {}}, {"no": "name"}])
    assert mlx_lm.extract_tool_calls(mixed) == (mixed, [])


def test_json_with_leading_prose_stays_text(mlx_lm):
    text = 'Sure: {"name": "read", "arguments": {}}'
    assert mlx_lm.extract_tool_calls(text) == (text, [])


def test_non_call_bare_json_stays_text(mlx_lm):
    text = '{"answer": 42}'
    assert mlx_lm.extract_tool_calls(text) == (text, [])


def test_bare_json_without_an_arguments_key_stays_text(mlx_lm):
    text = '{"name": "config"}'
    assert mlx_lm.extract_tool_calls(text) == (text, [])
    listed = '[{"name": "a"}]'
    assert mlx_lm.extract_tool_calls(listed) == (listed, [])


def test_marked_formats_still_default_missing_arguments(mlx_lm):
    remaining, calls = mlx_lm.extract_tool_calls('<tool_call>{"name": "ping"}</tool_call>')
    assert remaining == ""
    assert calls == [{"name": "ping", "arguments": {}}]
