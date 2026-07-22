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


def test_tool_specs_absent_or_malformed_returns_empty(mlx_lm):
    assert mlx_lm.tool_specs({}) == []
    assert mlx_lm.tool_specs({"tools": "read"}) == []
    assert mlx_lm.tool_specs({"tools": [{"description": "no name"}, "junk", 5]}) == []


def test_tool_specs_keeps_named_tools(mlx_lm):
    tools = [{"name": "read", "parameters": {}}, {"description": "nameless"}]
    assert mlx_lm.tool_specs({"tools": tools}) == [{"name": "read", "parameters": {}}]


def test_wrap_tools_produces_the_openai_function_form(mlx_lm):
    wrapped = mlx_lm.wrap_tools([{"name": "read", "parameters": {"type": "object"}}])
    assert wrapped == [
        {
            "type": "function",
            "function": {
                "name": "read",
                "description": "",
                "parameters": {"type": "object"},
            },
        }
    ]


def test_shape_tool_messages_rewires_assistant_calls_and_tool_results(mlx_lm):
    messages = [
        {"role": "user", "content": "hi"},
        {
            "role": "assistant",
            "content": "",
            "tool_calls": [{"id": "call_1", "name": "read", "arguments": {"path": "a"}}],
        },
        {"role": "tool", "tool_name": "read", "content": "data"},
    ]
    shaped = mlx_lm.shape_tool_messages(messages)
    assert shaped[0] == {"role": "user", "content": "hi"}
    assert shaped[1]["tool_calls"] == [
        {
            "type": "function",
            "id": "call_1",
            "function": {"name": "read", "arguments": {"path": "a"}},
        }
    ]
    assert shaped[2] == {"role": "tool", "name": "read", "content": "data"}


def test_shape_tool_messages_leaves_plain_messages_and_input_untouched(mlx_lm):
    messages = [{"role": "user", "content": "hi"}, "junk"]
    assert mlx_lm.shape_tool_messages(messages) == messages
    with_calls = [{"role": "assistant", "tool_calls": [{"name": "f", "arguments": {}}]}]
    mlx_lm.shape_tool_messages(with_calls)
    assert with_calls[0]["tool_calls"] == [{"name": "f", "arguments": {}}]


def test_tool_system_block_describes_tools_and_the_call_format(mlx_lm):
    block = mlx_lm.tool_system_block(
        [{"name": "read", "description": "Read a file.", "parameters": {"type": "object"}}]
    )
    assert "- read: Read a file." in block
    assert '{"type": "object"}' in block
    assert "<tool_call>" in block and "</tool_call>" in block


class FakeTokenizer:
    """apply_chat_template stand-in: renders a comparable string, optionally
    using the tools kwarg, optionally rejecting it like an old signature."""

    def __init__(self, uses_tools=True, rejects_tools=False):
        self.uses_tools = uses_tools
        self.rejects_tools = rejects_tools

    def apply_chat_template(self, messages, tools=None, **kwargs):
        if tools is not None and self.rejects_tools:
            raise TypeError("unexpected keyword argument 'tools'")
        rendered = "|".join(
            "{}:{}".format(m.get("role", ""), m.get("content", "")) for m in messages
        )
        if tools is not None and self.uses_tools:
            rendered += f"|tools:{len(tools)}"
        return rendered


def test_chat_prompt_without_tools_renders_plainly(mlx_lm):
    prompt = mlx_lm.chat_prompt(FakeTokenizer(), [{"role": "user", "content": "hi"}], [], {})
    assert prompt == "user:hi"


def test_chat_prompt_uses_the_templates_own_tool_rendering(mlx_lm):
    tools = [{"name": "read"}]
    prompt = mlx_lm.chat_prompt(FakeTokenizer(), [{"role": "user", "content": "hi"}], tools, {})
    assert prompt == "user:hi|tools:1"


def test_chat_prompt_falls_back_to_a_system_block_when_the_template_ignores_tools(mlx_lm):
    tools = [{"name": "read"}]
    prompt = mlx_lm.chat_prompt(
        FakeTokenizer(uses_tools=False), [{"role": "user", "content": "hi"}], tools, {}
    )
    assert prompt.startswith("system:You can call tools.")
    assert prompt.endswith("|user:hi")


def test_chat_prompt_falls_back_when_the_tokenizer_rejects_the_tools_kwarg(mlx_lm):
    tools = [{"name": "read"}]
    prompt = mlx_lm.chat_prompt(
        FakeTokenizer(rejects_tools=True), [{"role": "user", "content": "hi"}], tools, {}
    )
    assert prompt.startswith("system:You can call tools.")


def test_render_chatml_prepends_a_tool_system_block(mlx_lm):
    tools = [{"name": "read"}]
    prompt = mlx_lm.render_chatml([{"role": "user", "content": "hi"}], tools)
    assert prompt.startswith("<|im_start|>system\nYou can call tools.")
    assert prompt.endswith("<|im_start|>user\nhi<|im_end|>\n<|im_start|>assistant\n")


def test_render_chatml_without_tools_is_unchanged(mlx_lm):
    prompt = mlx_lm.render_chatml([{"role": "user", "content": "hi"}])
    assert prompt == "<|im_start|>user\nhi<|im_end|>\n<|im_start|>assistant\n"


def test_with_tool_block_merges_into_an_existing_system_turn(mlx_lm):
    messages = [
        {"role": "system", "content": "Be brief."},
        {"role": "user", "content": "hi"},
    ]
    blocked = mlx_lm.with_tool_block(messages, [{"name": "read"}])
    assert len(blocked) == 2
    assert blocked[0]["role"] == "system"
    assert blocked[0]["content"].startswith("Be brief.\n\nYou can call tools.")
    assert blocked[1] == {"role": "user", "content": "hi"}
    assert messages[0]["content"] == "Be brief."


def test_chat_prompt_fallback_keeps_a_single_system_turn(mlx_lm):
    messages = [
        {"role": "system", "content": "Be brief."},
        {"role": "user", "content": "hi"},
    ]
    prompt = mlx_lm.chat_prompt(FakeTokenizer(uses_tools=False), messages, [{"name": "read"}], {})
    assert prompt.count("system:") == 1
    assert prompt.startswith("system:Be brief.\n\nYou can call tools.")


class TokenIdTokenizer:
    """Renders to token-id lists like a real tokenizer with tokenize=True."""

    def apply_chat_template(self, messages, tools=None, **kwargs):
        ids = [len(messages)]
        if tools is not None:
            ids.append(len(tools))
        return ids


def test_chat_prompt_compares_token_id_renders(mlx_lm):
    prompt = mlx_lm.chat_prompt(
        TokenIdTokenizer(), [{"role": "user", "content": "hi"}], [{"name": "read"}], {}
    )
    assert prompt == [1, 1]


def test_render_chatml_inlines_assistant_tool_calls(mlx_lm):
    messages = mlx_lm.shape_tool_messages(
        [
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [{"id": "c1", "name": "read", "arguments": {"path": "a"}}],
            },
            {"role": "tool", "tool_name": "read", "content": "data"},
        ]
    )
    prompt = mlx_lm.render_chatml(messages)
    assert (
        '<|im_start|>assistant\n<tool_call>{"name": "read", "arguments": {"path": "a"}}'
        "</tool_call><|im_end|>\n" in prompt
    )
    assert "<|im_start|>tool\ndata<|im_end|>" in prompt


def test_render_chatml_merges_tools_into_an_existing_system_turn(mlx_lm):
    messages = [
        {"role": "system", "content": "Be brief."},
        {"role": "user", "content": "hi"},
    ]
    prompt = mlx_lm.render_chatml(messages, [{"name": "read"}])
    assert prompt.count("<|im_start|>system") == 1
    assert prompt.startswith("<|im_start|>system\nBe brief.\n\nYou can call tools.")
