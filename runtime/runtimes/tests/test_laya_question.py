"""Tests for how the laya sidecar reads a typed question out of a chat
request. The model answers `{"state", "questions"}` and nothing else, so what
reaches it has to be dug out of the last user message, and anything that is not
a question has to fail with a message that says what one looks like.
"""

import json

import pytest

QUESTION = {"type": "noul", "instructions": "Is this an outage?"}


def user(content):
    return {"role": "user", "content": content}


def test_the_question_comes_from_the_last_user_message(laya):
    messages = [
        {"role": "system", "content": "a default system prompt"},
        user("an earlier turn"),
        {"role": "assistant", "content": "{}"},
        user(json.dumps({"state": "500s", "questions": {"q": QUESTION}})),
    ]
    assert laya.parse_question(laya.last_user_text(messages)) == ("500s", {"q": QUESTION})


def test_content_parts_are_joined_into_the_text(laya):
    body = json.dumps({"state": "s", "questions": {"q": QUESTION}})
    parts = [{"type": "text", "text": body[:10]}, {"type": "text", "text": body[10:]}]
    assert laya.last_user_text([user(parts)]) == body


def test_a_single_question_is_keyed_as_question(laya):
    state, questions = laya.parse_question(json.dumps({"state": {"a": 1}, "question": QUESTION}))
    assert state == {"a": 1}
    assert questions == {"question": QUESTION}


@pytest.mark.parametrize(
    ("text", "reason"),
    [
        ("hello there", "not prose"),
        (json.dumps({"questions": {"q": QUESTION}}), 'no "state"'),
        (json.dumps({"state": "s"}), 'no "questions"'),
        (json.dumps({"state": "s", "questions": {"q": {"type": "essay"}}}), "no type of"),
        (json.dumps({"state": "s", "questions": {"q": {"type": "noul"}}}), "no instructions"),
    ],
)
def test_anything_else_is_refused_with_the_reason(laya, text, reason):
    with pytest.raises(ValueError, match=reason):
        laya.parse_question(text)


def test_pinned_code_is_run_and_anything_else_is_refused(laya, tmp_path):
    import hashlib
    import sys

    source = b"ANSWER = 42\n"
    (tmp_path / "judge_head.py").write_bytes(source)
    pin = (("laya_test_head", "judge_head.py", hashlib.sha256(source).hexdigest()),)
    try:
        assert laya.load_pinned(str(tmp_path), pin).ANSWER == 42

        (tmp_path / "judge_head.py").write_bytes(b"ANSWER = 'swapped'\n")
        with pytest.raises(RuntimeError, match="not the code this runtime was reviewed against"):
            laya.load_pinned(str(tmp_path), pin)
    finally:
        sys.modules.pop("laya_test_head", None)
