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


INKLING_VOCAB = {
    token: 200000 + index
    for index, token in enumerate(
        (
            "<|message_user|>",
            "<|message_model|>",
            "<|content_text|>",
            "<|content_thinking|>",
            "<|end_message|>",
            "<|content_model_end_sampling|>",
            "<|content_invoke_tool_json|>",
            "<|content_invoke_tool_text|>",
        )
    )
}


class InklingTokenizer:
    def __init__(self, vocab):
        self.vocab = {"plain": 1, **vocab}
        self.all_special_tokens = list(vocab)
        self.all_special_ids = list(vocab.values())

    def get_vocab(self):
        return self.vocab


def inkling_steps(*pieces):
    """A generation as stream_generate yields it: one step per token, a
    special token's text being the token itself, then the final step that
    repeats the last token."""
    steps = [(INKLING_VOCAB.get(piece, 1), piece) for piece in pieces]
    return [*steps, (steps[-1][0], "")]


def read_inkling(mlx_vlm, steps, tools=()):
    protocol = mlx_vlm.inkling.Protocol.of(InklingTokenizer(INKLING_VOCAB))
    reader = mlx_vlm.inkling.Reader(protocol, [{"name": name} for name in tools])
    events = []
    for index, (token, text) in enumerate(steps):
        events += reader.feed(token, text, final=index == len(steps) - 1)
    return events


def test_inkling_is_recognized_by_its_message_tokens(mlx_vlm):
    assert mlx_vlm.inkling.Protocol.of(InklingTokenizer(INKLING_VOCAB)) is not None
    partial = {k: v for k, v in INKLING_VOCAB.items() if k != "<|content_thinking|>"}
    assert mlx_vlm.inkling.Protocol.of(InklingTokenizer(partial)) is None
    assert mlx_vlm.inkling.Protocol.of(InklingTokenizer({})) is None


def test_a_tokenizer_that_cannot_list_its_vocabulary_is_not_inkling(mlx_vlm):
    assert mlx_vlm.inkling.Protocol.of(object()) is None


def test_inkling_keeps_an_image_with_its_original_message(mlx_vlm, tmp_path):
    messages = [
        {"role": "user", "content": "red?", "images": ["aW1hZ2U="]},
        {"role": "assistant", "content": "yes"},
        {"role": "user", "content": "what shade?"},
    ]
    shaped, paths = mlx_vlm.materialize_images(messages, str(tmp_path), mark_images=True)
    assert len(paths) == 1
    assert shaped[0]["content"] == [{"type": "image"}, {"type": "text", "text": "red?"}]
    assert shaped[2] == messages[2]


def test_inkling_marks_an_image_only_message_without_an_empty_text_block(mlx_vlm, tmp_path):
    messages = [{"role": "user", "content": "", "images": ["aW1hZ2U="]}]
    shaped, _ = mlx_vlm.materialize_images(messages, str(tmp_path), mark_images=True)
    assert shaped[0]["content"] == [{"type": "image"}]


def test_inkling_template_reads_tool_history(mlx_vlm):
    messages = [
        {"role": "user", "content": "colour?"},
        {
            "role": "assistant",
            "content": None,
            "tool_calls": [{"id": "call_1", "name": "color", "arguments": {"of": "sky"}}],
        },
        {"role": "tool", "tool_name": "color", "content": "blue"},
        {"role": "tool", "tool_call_id": "call_1", "content": "still blue"},
    ]
    _, assistant, named, by_id = mlx_vlm.inkling.template_messages(messages)
    assert assistant == {
        "role": "assistant",
        "content": "",
        "tool_calls": [
            {
                "id": "call_1",
                "type": "function",
                "function": {"name": "color", "arguments": {"of": "sky"}},
            }
        ],
    }
    assert named == {"role": "tool", "name": "color", "content": "blue"}
    # The template names a result by the id of the call it answers.
    assert by_id == messages[3]


def test_inkling_runs_a_turn_with_thinking_off_at_zero_effort(mlx_vlm):
    assert mlx_vlm.inkling.template_kwargs({"thinking": False}) == {"reasoning_effort": "none"}
    assert mlx_vlm.inkling.template_kwargs({"thinking": True}) == {}
    assert mlx_vlm.inkling.template_kwargs({}) == {}


def test_inkling_streams_thinking_and_text_by_block(mlx_vlm):
    steps = inkling_steps(
        "<|message_model|>",
        "<|content_thinking|>",
        "First",
        "<|end_message|>",
        "<|message_model|>",
        "<|content_text|>",
        "O",
        "K",
        "<|end_message|>",
    )
    assert read_inkling(mlx_vlm, steps) == [
        {"event": "thinking", "text": "First"},
        {"event": "text", "text": "O"},
        {"event": "text", "text": "K"},
    ]


def test_inkling_reads_markers_by_token_not_by_text(mlx_vlm):
    # The words of a marker in ordinary text are text, and a step that flushes
    # held text along with a marker keeps that text in the block it closes.
    steps = [
        (INKLING_VOCAB["<|message_model|>"], "<|message_model|>"),
        (INKLING_VOCAB["<|content_text|>"], "<|content_text|>"),
        (1, "say <|end_message|>"),
        (INKLING_VOCAB["<|end_message|>"], "é<|end_message|>"),
        (INKLING_VOCAB["<|end_message|>"], ""),
    ]
    assert read_inkling(mlx_vlm, steps) == [
        {"event": "text", "text": "say <|end_message|>"},
        {"event": "text", "text": "é"},
    ]


def test_inkling_flushes_the_text_held_at_the_end(mlx_vlm):
    steps = [
        (INKLING_VOCAB["<|message_model|>"], "<|message_model|>"),
        (INKLING_VOCAB["<|content_text|>"], "<|content_text|>"),
        (1, "caf"),
        (1, "é"),
    ]
    assert read_inkling(mlx_vlm, steps) == [
        {"event": "text", "text": "caf"},
        {"event": "text", "text": "é"},
    ]


def test_inkling_assembles_a_tool_call_once_its_block_closes(mlx_vlm):
    steps = inkling_steps(
        "<|message_model|>",
        "color",
        "<|content_invoke_tool_json|>",
        '{"name":"color",',
        '"args":{"id":1}}',
        "<|end_message|>",
    )
    assert read_inkling(mlx_vlm, steps, tools=["color"]) == [
        {"event": "tool_call", "name": "color", "arguments": {"id": 1}}
    ]


def test_inkling_drops_invocations_that_are_not_offered_calls(mlx_vlm):
    def invocation(header, body, close=True):
        pieces = ["<|message_model|>", header, "<|content_invoke_tool_json|>", body]
        return inkling_steps(*pieces, *(["<|end_message|>"] if close else []))

    for steps in (
        invocation("color", '{"name":"color","args":[1]}'),
        invocation("other", '{"name":"other","args":{}}'),
        invocation("other", '{"name":"color","args":{}}'),
        invocation("color", "not JSON"),
        invocation("color", '{"name":"color","args":{}}', close=False),
    ):
        assert read_inkling(mlx_vlm, steps, tools=["color"]) == []
    assert read_inkling(mlx_vlm, invocation("color", '{"name":"color","args":{}}')) == []


def test_inkling_drops_blocks_it_has_no_event_for(mlx_vlm):
    steps = inkling_steps(
        "<|message_model|>",
        "<|content_invoke_tool_text|>",
        "color(sky)",
        "<|end_message|>",
        "<|message_user|>",
        "<|content_text|>",
        "hi",
        "<|end_message|>",
    )
    assert read_inkling(mlx_vlm, steps, tools=["color"]) == []


def test_inkling_reads_on_once_the_model_speaks_again(mlx_vlm):
    steps = inkling_steps(
        "<|message_user|>",
        "<|content_text|>",
        "hi",
        "<|end_message|>",
        "<|message_model|>",
        "<|content_text|>",
        "hello",
        "<|end_message|>",
    )
    assert read_inkling(mlx_vlm, steps) == [{"event": "text", "text": "hello"}]


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
    using the tools kwarg, optionally rejecting it like an old signature or
    rejecting a system role like a strict template."""

    def __init__(self, uses_tools=True, rejects_tools=False, rejects_system=False):
        self.uses_tools = uses_tools
        self.rejects_tools = rejects_tools
        self.rejects_system = rejects_system

    def apply_chat_template(self, messages, tools=None, **kwargs):
        if tools is not None and self.rejects_tools:
            raise TypeError("unexpected keyword argument 'tools'")
        if self.rejects_system and any(m.get("role") == "system" for m in messages):
            raise ValueError("System role not supported")
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


def test_vlm_inline_tool_history_folds_calls_into_assistant_text(mlx_vlm):
    messages = [
        {
            "role": "assistant",
            "content": "checking",
            "tool_calls": [{"id": "c1", "name": "read", "arguments": {"path": "a"}}],
        }
    ]
    shaped = mlx_vlm.inline_tool_history(messages)
    assert shaped == [
        {
            "role": "assistant",
            "content": 'checking\n<tool_call>{"name": "read", "arguments": {"path": "a"}}'
            "</tool_call>",
        }
    ]
    assert "tool_calls" in messages[0]


def test_vlm_inline_tool_history_turns_tool_results_into_user_turns(mlx_vlm):
    shaped = mlx_vlm.inline_tool_history([{"role": "tool", "tool_name": "read", "content": "data"}])
    assert shaped == [{"role": "user", "content": '[tool "read" result]\ndata'}]


def test_vlm_inline_tool_history_leaves_plain_messages_untouched(mlx_vlm):
    messages = [{"role": "user", "content": "hi"}, "junk"]
    assert mlx_vlm.inline_tool_history(messages) == messages


def test_vlm_with_tool_block_merges_into_an_existing_system_turn(mlx_vlm):
    messages = [
        {"role": "system", "content": "Be brief."},
        {"role": "user", "content": "hi"},
    ]
    blocked = mlx_vlm.with_tool_block(messages, [{"name": "read"}])
    assert len(blocked) == 2
    assert blocked[0]["content"].startswith("Be brief.\n\nYou can call tools.")


def test_vlm_extraction_matches_the_mlx_lm_copy(mlx_lm, mlx_vlm):
    text = 'ok <tool_call>{"name": "read", "arguments": {}}</tool_call>'
    assert mlx_vlm.extract_tool_calls(text) == mlx_lm.extract_tool_calls(text)
    assert mlx_vlm.extract_tool_calls("plain") == ("plain", [])


def test_chat_prompt_folds_tools_into_a_user_turn_when_system_is_rejected(mlx_lm):
    tokenizer = FakeTokenizer(uses_tools=False, rejects_system=True)
    prompt = mlx_lm.chat_prompt(
        tokenizer, [{"role": "user", "content": "hi"}], [{"name": "read"}], {}
    )
    assert prompt.startswith("user:You can call tools.")
    assert prompt.endswith("hi")
    assert "system:" not in prompt


def test_with_tool_block_in_user_appends_a_turn_when_no_user_exists(mlx_lm):
    shaped = mlx_lm.with_tool_block_in_user(
        [{"role": "assistant", "content": "x"}], [{"name": "f"}]
    )
    assert shaped[-1]["role"] == "user"
    assert shaped[-1]["content"].startswith("You can call tools.")


def test_vlm_inline_tool_history_folds_results_into_one_user_turn(mlx_vlm):
    messages = [
        {"role": "user", "content": "go"},
        {
            "role": "assistant",
            "content": "",
            "tool_calls": [
                {"name": "a", "arguments": {}},
                {"name": "b", "arguments": {}},
            ],
        },
        {"role": "tool", "tool_name": "a", "content": "one"},
        {"role": "tool", "tool_name": "b", "content": "two"},
    ]
    shaped = mlx_vlm.inline_tool_history(messages)
    assert [m["role"] for m in shaped] == ["user", "assistant", "user"]
    assert shaped[2]["content"] == '[tool "a" result]\none\n\n[tool "b" result]\ntwo'


def test_vlm_build_prompt_without_tools_renders_plainly(mlx_vlm):
    prompt = mlx_vlm.build_prompt(len, [{"role": "user", "content": "hi"}], [])
    assert prompt == 1


def test_vlm_build_prompt_prefers_the_system_block(mlx_vlm):
    def render(messages):
        return "|".join(f"{m.get('role')}:{m.get('content')}" for m in messages)

    prompt = mlx_vlm.build_prompt(render, [{"role": "user", "content": "hi"}], [{"name": "read"}])
    assert prompt.startswith("system:You can call tools.")
    assert prompt.endswith("|user:hi")


def test_vlm_build_prompt_folds_tools_into_a_user_turn_when_system_is_rejected(mlx_vlm):
    def render(messages):
        if any(m.get("role") == "system" for m in messages):
            raise ValueError("System role not supported")
        return "|".join(f"{m.get('role')}:{m.get('content')}" for m in messages)

    prompt = mlx_vlm.build_prompt(render, [{"role": "user", "content": "hi"}], [{"name": "read"}])
    assert prompt.startswith("user:You can call tools.")
    assert "system:" not in prompt


class _LastResponse:
    prompt_tokens = 30
    prompt_tps = 300.0
    generation_tokens = 4
    generation_tps = 0.0


def test_phase_timings_divides_each_phase_by_its_rate_and_skips_a_phase_with_no_rate(mlx_lm):
    assert mlx_lm.phase_timings(_LastResponse()) == {"prompt_seconds": 0.1}
    assert mlx_lm.phase_timings(None) == {}


def test_phase_timings_skips_a_rate_that_is_not_a_positive_finite_number(mlx_lm):
    class Last:
        prompt_tokens = 30
        prompt_tps = float("nan")
        generation_tokens = 4
        generation_tps = float("inf")

    # A NaN would otherwise reach json.dumps as a bare `NaN` and cost the whole
    # done frame, not just the figure.
    assert mlx_lm.phase_timings(Last()) == {}


def test_vlm_phase_timings_matches_the_mlx_lm_copy(mlx_lm, mlx_vlm):
    assert mlx_vlm.phase_timings(_LastResponse()) == mlx_lm.phase_timings(_LastResponse())
    assert mlx_vlm.phase_timings(None) == mlx_lm.phase_timings(None)
