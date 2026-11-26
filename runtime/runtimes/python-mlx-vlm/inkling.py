"""Inkling's native message protocol.

Inkling checkpoints speak in typed message blocks rather than one free-text
turn: a model message opens with `<|message_model|>`, names its block with a
content token (text, thinking, a JSON tool invocation) and closes with
`<|end_message|>`. The checkpoint's chat template renders requests in that
form, so this module only reshapes Hedos's turns into what the template reads
and reads the generated blocks back by their token ids.
"""

import json

MESSAGE = "<|message_"
MESSAGE_MODEL = "<|message_model|>"
END_MESSAGE = "<|end_message|>"
BLOCKS = {
    "<|content_text|>": "text",
    "<|content_thinking|>": "thinking",
    "<|content_invoke_tool_json|>": "tool_call",
}
# A tokenizer carrying all of these speaks the protocol, whatever the
# checkpoint's format or model type.
REQUIRED = (MESSAGE_MODEL, END_MESSAGE, "<|content_model_end_sampling|>", *BLOCKS)


class Protocol:
    """The protocol's special tokens as one tokenizer numbers them."""

    def __init__(self, specials):
        self.specials = specials

    @classmethod
    def of(cls, tokenizer):
        """The protocol `tokenizer` speaks, or None when it is not Inkling's."""
        # Every model this sidecar loads is asked, and not every processor's
        # tokenizer can list its vocabulary: that one is simply not Inkling.
        get_vocab = getattr(tokenizer, "get_vocab", None)
        vocab = get_vocab() if callable(get_vocab) else None
        if not isinstance(vocab, dict) or any(token not in vocab for token in REQUIRED):
            return None
        special_ids = set(getattr(tokenizer, "all_special_ids", ()))
        specials = {
            vocab[token]: token
            for token in getattr(tokenizer, "all_special_tokens", ())
            if token in vocab and vocab[token] in special_ids
        }
        specials.update({vocab[token]: token for token in REQUIRED})
        return cls(specials)


def template_messages(messages):
    """Hedos chat turns in the shape Inkling's chat template reads."""
    shaped = []
    for message in messages:
        message = dict(message)
        if message.get("content") is None:
            message["content"] = ""
        if calls := message.pop("tool_calls", None):
            message["tool_calls"] = [
                {
                    **({"id": call["id"]} if call.get("id") else {}),
                    "type": "function",
                    "function": {"name": call["name"], "arguments": call.get("arguments") or {}},
                }
                for call in calls
                if isinstance(call, dict) and call.get("name")
            ]
        if message.get("role") == "tool" and (name := message.pop("tool_name", None)):
            message["name"] = name
        shaped.append(message)
    return shaped


def template_kwargs(request):
    """The template switches a request sets: a turn with thinking turned off
    runs at the template's zero effort level."""
    if request.get("thinking") is False:
        return {"reasoning_effort": "none"}
    return {}


class Reader:
    """Turns the generated token stream back into Hedos events: text and
    thinking as they arrive, and each tool invocation once its block closes.

    Only blocks of a model message are read: a user, system or tool message
    the model goes on to write is not its reply. An invocation becomes a call
    only when it is a complete JSON object naming one of the offered tools,
    with an object for its arguments; anything else, and every block of a kind
    Hedos has no event for, is dropped.
    """

    def __init__(self, protocol, tools):
        self.specials = protocol.specials
        self.tool_names = {tool["name"] for tool in tools}
        # The prompt ends where the model's turn begins.
        self.speaking = True
        self.kind = None
        self.header = []
        self.body = []

    def feed(self, token, text, final=False):
        """Read one generation step. The last step of a stream repeats its
        final token with whatever text the detokenizer still held, so it is
        read as text only."""
        marker = None if final else self.specials.get(token)
        if marker is None:
            return self.route(text)
        events = self.route(text.removesuffix(marker))
        if marker.startswith(MESSAGE):
            self.speaking = marker == MESSAGE_MODEL
            self.kind = "header" if self.speaking else "ignored"
            self.header.clear()
        elif not self.speaking:
            self.kind = "ignored"
        elif marker in BLOCKS:
            self.kind = BLOCKS[marker]
            self.body.clear()
        elif marker == END_MESSAGE:
            if self.kind == "tool_call" and (call := self.tool_call()) is not None:
                events.append(call)
            self.kind = None
        else:
            self.kind = "ignored"
        return events

    def route(self, text):
        if not text:
            return []
        if self.kind in ("text", "thinking"):
            return [{"event": self.kind, "text": text}]
        if self.kind == "header":
            self.header.append(text)
        elif self.kind == "tool_call":
            self.body.append(text)
        return []

    def tool_call(self):
        try:
            invocation = json.loads("".join(self.body))
        except ValueError:
            return None
        if not isinstance(invocation, dict):
            return None
        name, arguments = invocation.get("name"), invocation.get("args")
        header = "".join(self.header).strip()
        if (
            not isinstance(name, str)
            or name not in self.tool_names
            or (header and header != name)
            or not isinstance(arguments, dict)
        ):
            return None
        return {"event": "tool_call", "name": name, "arguments": arguments}
