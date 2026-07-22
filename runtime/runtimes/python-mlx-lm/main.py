import argparse
import json
import os
import select
import struct
import time

real_stdout = os.dup(1)
os.dup2(2, 1)


def send(frame_type, payload):
    os.write(real_stdout, struct.pack("<I", len(payload) + 1) + bytes([frame_type]) + payload)


def send_json(obj):
    send(1, json.dumps(obj).encode())


def read_exact(count):
    buffer = b""
    while len(buffer) < count:
        chunk = os.read(0, count - len(buffer))
        if not chunk:
            return None
        buffer += chunk
    return buffer


def read_frame():
    header = read_exact(4)
    if header is None:
        return None
    (length,) = struct.unpack("<I", header)
    body = read_exact(length)
    if body is None:
        return None
    return body[0], body[1:]


def read_request():
    frame = read_frame()
    if frame is None:
        return None
    _, payload = frame
    try:
        return json.loads(payload)
    except ValueError:
        return {}


def pending_op():
    if not select.select([0], [], [], 0)[0]:
        return None
    request = read_request()
    if request is None:
        return "shutdown"
    return request.get("op")


def sampler_kwargs(request):
    kwargs = {}
    if "temperature" in request:
        kwargs["temp"] = float(request["temperature"])
    if "top_p" in request:
        kwargs["top_p"] = float(request["top_p"])
    if "min_p" in request:
        kwargs["min_p"] = float(request["min_p"])
    if "top_k" in request:
        kwargs["top_k"] = int(request["top_k"])
    return kwargs


def stop_strings(request):
    stops = request.get("stop")
    if stops is None:
        return []
    if isinstance(stops, str):
        return [stops]
    return [s for s in stops if isinstance(s, str) and s]


class StopScanner:
    def __init__(self, stops):
        self.stops = [s for s in stops if s]
        self.buffer = ""
        self.stopped = False

    @property
    def active(self):
        return bool(self.stops)

    def feed(self, chunk):
        if not self.stops or self.stopped:
            return "" if self.stopped else chunk
        self.buffer += chunk
        earliest = None
        for stop in self.stops:
            index = self.buffer.find(stop)
            if index != -1 and (earliest is None or index < earliest):
                earliest = index
        if earliest is not None:
            emit = self.buffer[:earliest]
            self.buffer = ""
            self.stopped = True
            return emit
        longest = max((len(s) for s in self.stops), default=1) - 1
        cap = min(longest, len(self.buffer))
        hold = 0
        for length in range(cap, 0, -1):
            suffix = self.buffer[-length:]
            if any(s.startswith(suffix) for s in self.stops):
                hold = length
                break
        cut = len(self.buffer) - hold
        emit = self.buffer[:cut]
        self.buffer = self.buffer[cut:]
        return emit

    def flush(self):
        if self.stopped:
            return ""
        emit = self.buffer
        self.buffer = ""
        return emit


class ThinkSplitter:
    PAIRS = [("<think>", "</think>"), ("<|START_THINKING|>", "<|END_THINKING|>")]

    def __init__(self):
        self.mode = "text"
        self.close = None
        self.buffer = ""
        self.open_tags = [open_tag for open_tag, _ in self.PAIRS]

    def feed(self, chunk):
        self.buffer += chunk
        out = []
        while True:
            if self.mode == "text":
                earliest = None
                for open_tag, close_tag in self.PAIRS:
                    index = self.buffer.find(open_tag)
                    if index != -1 and (earliest is None or index < earliest[0]):
                        earliest = (index, open_tag, close_tag)
                if earliest is None:
                    emit = self._emittable(self.open_tags)
                    if emit:
                        out.append(("text", emit))
                        self.buffer = self.buffer[len(emit) :]
                    break
                index, open_tag, close_tag = earliest
                before = self.buffer[:index]
                if before:
                    out.append(("text", before))
                self.buffer = self.buffer[index + len(open_tag) :]
                self.mode = "thinking"
                self.close = close_tag
            else:
                index = self.buffer.find(self.close)
                if index == -1:
                    emit = self._emittable([self.close])
                    if emit:
                        out.append(("thinking", emit))
                        self.buffer = self.buffer[len(emit) :]
                    break
                before = self.buffer[:index]
                if before:
                    out.append(("thinking", before))
                self.buffer = self.buffer[index + len(self.close) :]
                self.mode = "text"
                self.close = None
        return out

    def flush(self):
        if not self.buffer:
            return []
        kind = "thinking" if self.mode == "thinking" else "text"
        out = [(kind, self.buffer)]
        self.buffer = ""
        return out

    def _emittable(self, tags):
        longest = max((len(tag) for tag in tags), default=1)
        max_hold = min(longest - 1, len(self.buffer))
        for length in range(max_hold, 0, -1):
            suffix = self.buffer[-length:]
            if any(tag.startswith(suffix) for tag in tags):
                return self.buffer[: len(self.buffer) - length]
        return self.buffer


NO_TEMPLATE_NOTICE = "this model has no chat template — using a generic format"


def tool_specs(request):
    tools = request.get("tools")
    if not isinstance(tools, list):
        return []
    return [tool for tool in tools if isinstance(tool, dict) and tool.get("name")]


def wrap_tools(tools):
    return [
        {
            "type": "function",
            "function": {
                "name": tool.get("name", ""),
                "description": tool.get("description", ""),
                "parameters": tool.get("parameters", {}),
            },
        }
        for tool in tools
    ]


def shape_tool_messages(messages):
    shaped = []
    for message in messages:
        if not isinstance(message, dict):
            shaped.append(message)
            continue
        message = dict(message)
        calls = message.get("tool_calls")
        if isinstance(calls, list) and calls:
            message["tool_calls"] = [
                {
                    "type": "function",
                    "id": str(call.get("id", "")),
                    "function": {
                        "name": call.get("name", ""),
                        "arguments": call.get("arguments", {}),
                    },
                }
                for call in calls
                if isinstance(call, dict)
            ]
        if "tool_name" in message:
            message["name"] = message.pop("tool_name")
        shaped.append(message)
    return shaped


def tool_system_block(tools):
    lines = ["You can call tools. The available tools are:", ""]
    for tool in tools:
        lines.append(
            "- {}: {} Parameters schema: {}".format(
                tool.get("name", ""),
                tool.get("description", ""),
                json.dumps(tool.get("parameters", {})),
            )
        )
    lines.append("")
    lines.append(
        "To call a tool, reply with exactly one block of the form "
        '<tool_call>{"name": "<tool name>", "arguments": {…}}</tool_call> '
        "and nothing after it. Only call a tool when it is needed to answer."
    )
    return "\n".join(lines)


def with_tool_block(messages, tools):
    # Merged into an existing leading system turn rather than prepended as a
    # second one — strict templates reject more than one system message.
    block = tool_system_block(tools)
    if messages and isinstance(messages[0], dict) and messages[0].get("role") == "system":
        first = dict(messages[0])
        content = first.get("content", "")
        first["content"] = f"{content}\n\n{block}" if content else block
        return [first] + messages[1:]
    return [{"role": "system", "content": block}] + messages


def chat_prompt(tokenizer, messages, tools, template_kwargs):
    if not tools:
        return tokenizer.apply_chat_template(messages, **template_kwargs)
    try:
        with_tools = tokenizer.apply_chat_template(
            messages, tools=wrap_tools(tools), **template_kwargs
        )
        # A template with no tools support renders the same prompt with and
        # without them; only a differing render proves the model saw the offer.
        if with_tools != tokenizer.apply_chat_template(messages, **template_kwargs):
            return with_tools
    except TypeError:
        pass
    return tokenizer.apply_chat_template(with_tool_block(messages, tools), **template_kwargs)


def chatml_content(message):
    content = message.get("content", "")
    calls = message.get("tool_calls")
    if not isinstance(calls, list):
        return content
    blocks = []
    for call in calls:
        function = call.get("function", {}) if isinstance(call, dict) else {}
        name = function.get("name")
        if not name:
            continue
        body = json.dumps({"name": name, "arguments": function.get("arguments", {})})
        blocks.append(f"<tool_call>{body}</tool_call>")
    if not blocks:
        return content
    return "\n".join([content] + blocks) if content else "\n".join(blocks)


def render_chatml(messages, tools=None):
    if tools:
        messages = with_tool_block(messages, tools)
    prompt = ""
    for message in messages:
        prompt += "<|im_start|>{}\n{}<|im_end|>\n".format(
            message.get("role", ""),
            chatml_content(message),
        )
    prompt += "<|im_start|>assistant\n"
    return prompt


CALL_OPEN = "<tool_call>"
CALL_CLOSE = "</tool_call>"
TOOL_CALLS_MARKER = "[TOOL_CALLS]"
PYTHON_TAG = "<|python_tag|>"


def try_json(text):
    try:
        return json.loads(text)
    except ValueError:
        return None


def call_from_value(value):
    if not isinstance(value, dict):
        return None
    name = value.get("name")
    if not isinstance(name, str) or not name:
        return None
    arguments = value.get("arguments", value.get("parameters"))
    if arguments is None:
        arguments = {}
    if not isinstance(arguments, dict):
        return None
    call = {"name": name, "arguments": arguments}
    if isinstance(value.get("id"), str) and value["id"]:
        call["id"] = value["id"]
    return call


def parse_tagged_calls(text):
    calls = []
    remaining = []
    cursor = 0
    while True:
        start = text.find(CALL_OPEN, cursor)
        if start == -1:
            remaining.append(text[cursor:])
            break
        end = text.find(CALL_CLOSE, start)
        if end == -1:
            remaining.append(text[cursor:])
            break
        remaining.append(text[cursor:start])
        call = call_from_value(try_json(text[start + len(CALL_OPEN) : end].strip()))
        if call:
            calls.append(call)
        else:
            remaining.append(text[start : end + len(CALL_CLOSE)])
        cursor = end + len(CALL_CLOSE)
    return "".join(remaining), calls


def parse_mistral_calls(text):
    start = text.find(TOOL_CALLS_MARKER)
    if start == -1:
        return text, []
    value = try_json(text[start + len(TOOL_CALLS_MARKER) :].strip())
    entries = value if isinstance(value, list) else [value]
    calls = [call for call in (call_from_value(entry) for entry in entries) if call]
    if not calls:
        return text, []
    return text[:start], calls


def parse_llama_calls(text):
    stripped = text.strip()
    if stripped.startswith(PYTHON_TAG):
        call = call_from_value(try_json(stripped[len(PYTHON_TAG) :].strip()))
        if call:
            return "", [call]
        return text, []
    value = try_json(stripped)
    if isinstance(value, list):
        calls = [call for call in (call_from_value(entry) for entry in value) if call]
        if calls and len(calls) == len(value):
            return "", calls
        return text, []
    call = call_from_value(value)
    if call:
        return "", [call]
    return text, []


def extract_tool_calls(text):
    # The marker formats are mutually exclusive, so a fixed order is safe; the
    # bare-JSON fallback (inside parse_llama_calls) must run last because any
    # model family can degrade to it.
    for parser in (parse_tagged_calls, parse_mistral_calls, parse_llama_calls):
        remaining, calls = parser(text)
        if calls:
            return remaining, calls
    return text, []


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--workdir", required=True)
    args = parser.parse_args()

    os.environ.setdefault("HF_HUB_OFFLINE", "1")

    import mlx.core as mx
    from mlx_lm import load, stream_generate
    from mlx_lm.sample_utils import make_logits_processors, make_sampler

    model, tokenizer = load(args.model)
    send_json({"event": "ready"})

    while True:
        request = read_request()
        if request is None:
            break
        op = request.get("op")
        if op == "shutdown":
            break
        if op == "ping":
            send_json({"event": "pong"})
            continue
        if op not in ("chat", "complete"):
            continue

        started = time.monotonic()
        cancelled = False
        shutdown_requested = False
        last = None
        try:
            no_template = False
            tools = []
            if op == "chat":
                messages = shape_tool_messages(request.get("messages", []))
                tools = tool_specs(request)
                if getattr(tokenizer, "chat_template", None):
                    template_kwargs = {"add_generation_prompt": True}
                    if request.get("thinking") is False:
                        template_kwargs["enable_thinking"] = False
                    try:
                        prompt = chat_prompt(tokenizer, messages, tools, template_kwargs)
                    except TypeError:
                        template_kwargs.pop("enable_thinking", None)
                        prompt = chat_prompt(tokenizer, messages, tools, template_kwargs)
                else:
                    no_template = True
                    prompt = render_chatml(messages, tools)
            else:
                prompt = request.get("prompt", "")
            max_tokens = int(request.get("max_tokens", 4096))
            if request.get("seed") is not None:
                mx.random.seed(int(request["seed"]))
            scanner = StopScanner(stop_strings(request))
            generate_kwargs = {
                "max_tokens": max_tokens,
                "sampler": make_sampler(**sampler_kwargs(request)),
            }
            if "repeat_penalty" in request:
                generate_kwargs["logits_processors"] = make_logits_processors(
                    repetition_penalty=float(request["repeat_penalty"])
                )
            if no_template:
                send_json({"event": "status", "message": NO_TEMPLATE_NOTICE})
            send_json({"event": "begin"})
            splitter = ThinkSplitter()
            # With tools offered, visible text is held to end-of-turn so a tool
            # call can be parsed out of the full reply; thinking still streams.
            collector = [] if tools else None
            stopped = False

            # scanner/splitter/collector are rebound each request; these closures
            # run synchronously within the same iteration, never a later one.
            def deliver_text(text):
                if collector is None:  # noqa: B023
                    send_json({"event": "text", "text": text})
                else:
                    collector.append(text)  # noqa: B023

            def emit_text(text):
                if not text:
                    return False
                if not scanner.active:  # noqa: B023
                    deliver_text(text)
                    return False
                emit = scanner.feed(text)  # noqa: B023
                if emit:
                    deliver_text(emit)
                return scanner.stopped  # noqa: B023

            def emit_raw(text):
                for kind, value in splitter.feed(text):  # noqa: B023
                    if kind == "thinking":
                        send_json({"event": "thinking", "text": value})
                    elif emit_text(value):
                        return True
                return False

            for response in stream_generate(model, tokenizer, prompt, **generate_kwargs):
                last = response
                if emit_raw(response.text):
                    stopped = True
                    break
                inner = pending_op()
                if inner in ("cancel", "shutdown"):
                    cancelled = True
                    shutdown_requested = inner == "shutdown"
                    break
            if cancelled:
                send_json({"event": "cancelled"})
                if shutdown_requested:
                    break
                continue
            if not stopped:
                for kind, value in splitter.flush():
                    if kind == "thinking":
                        send_json({"event": "thinking", "text": value})
                    else:
                        emit_text(value)
                if scanner.active:
                    tail = scanner.flush()
                    if tail:
                        deliver_text(tail)
            if collector is not None:
                remaining, calls = extract_tool_calls("".join(collector))
                if remaining and (not calls or remaining.strip()):
                    send_json({"event": "text", "text": remaining})
                for call in calls:
                    send_json({"event": "tool_call", **call})
            send_json(
                {
                    "event": "done",
                    "seconds": time.monotonic() - started,
                    "prompt_tokens": last.prompt_tokens if last else 0,
                    "completion_tokens": last.generation_tokens if last else 0,
                }
            )
        except Exception as error:
            send_json({"event": "error", "message": str(error)})


if __name__ == "__main__":
    main()
