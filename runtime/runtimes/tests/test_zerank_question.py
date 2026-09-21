"""Tests for how the zerank sidecar turns a typed question into the pairs a
reranker scores, and its logits back into the judgment envelope laya answers
with. No model is loaded: a stand-in returns fixed logits per pair.
"""

import json
import math

import pytest


class Scores:
    """Answers each (query, document) pair with the logit given for its document."""

    def __init__(self, by_document):
        self.by_document = by_document
        self.pairs = []

    def predict(self, pairs, convert_to_numpy=True):
        self.pairs.extend(pairs)
        return [self.by_document[document] for _, document in pairs]


def test_a_choice_scores_each_option_against_the_state_and_the_question(zerank):
    state, questions = zerank.parse_question(
        '{"state": "500 three times", "questions": {"route": {"type": "choice",'
        ' "instructions": "Which team?", "criteria": {"infra": "servers", "billing": ""}}}}'
    )
    model = Scores({"infra: servers": 5.0, "billing": -5.0})
    judgment = zerank.judge(model, *zerank.pairs_for(state, questions))
    assert model.pairs == [
        ("500 three times\n\nWhich team?", "infra: servers"),
        ("500 three times\n\nWhich team?", "billing"),
    ]
    route = judgment["answers"]["route"]
    assert route["choice"] == "infra"
    # softmax over logit / 5: e^1 / (e^1 + e^-1)
    assert route["probabilities"]["infra"] == round(1 / (1 + math.exp(-2)), 4)
    assert route["zerank"]["relevance"]["billing"] == round(1 / (1 + math.e), 4)


def test_a_choices_options_keep_the_order_they_were_written_in(zerank):
    text = json.dumps(
        {
            "state": "",
            "questions": {
                "q": {"type": "choice", "instructions": "i", "criteria": {"zeta": "", "alpha": ""}}
            },
        }
    )
    state, questions = zerank.parse_question(text)
    assert [key for key, _ in zerank.documents_for(state, questions["q"])] == ["zeta", "alpha"]


def test_a_listed_choice_scores_its_labels(zerank):
    question = {"type": "choice", "instructions": "i", "criteria": ["yes", "no"]}
    assert zerank.documents_for("", question) == [("yes", "yes"), ("no", "no")]


def test_a_score_is_the_expected_level(zerank):
    question = {"type": "score", "instructions": "How bad?", "criteria": ["fine", "bad"]}
    model = Scores({"fine": 0.0, "bad": 0.0})
    judgment = zerank.judge(model, *zerank.pairs_for("s", {"q": question}))
    answer = judgment["answers"]["q"]
    assert answer["score"] == 0.5
    assert answer["legend"] == {"0": "fine", "1": "bad"}
    assert answer["probabilities"] == {"0": 0.5, "1": 0.5}


def test_a_noul_scores_the_state_against_the_statement(zerank):
    question = {"type": "noul", "instructions": "Is this an outage?"}
    model = Scores({"500 three times": 0.0})
    judgment = zerank.judge(model, *zerank.pairs_for("500 three times", {"q": question}))
    assert model.pairs == [("Is this an outage?", "500 three times")]
    assert judgment["answers"]["q"]["noul"] == 0.5


def test_a_structured_state_is_scored_as_its_json(zerank):
    state, _ = zerank.parse_question(
        json.dumps({"state": {"errors": 3}, "question": {"type": "noul", "instructions": "i"}})
    )
    assert state == '{"errors": 3}'


@pytest.mark.parametrize(
    ("text", "reason"),
    [
        ("hello there", "not prose"),
        (json.dumps({"questions": {"q": {"type": "noul", "instructions": "i"}}}), 'no "state"'),
        (json.dumps({"state": "s"}), 'no "questions"'),
        (json.dumps({"state": "", "question": {"type": "noul", "instructions": "i"}}), "no state"),
        (
            json.dumps({"state": "s", "questions": {"q": {"type": "choice", "instructions": "i"}}}),
            "no criteria to choose between",
        ),
        (
            json.dumps(
                {
                    "state": "s",
                    "question": {
                        "type": "choice",
                        "instructions": "i",
                        "criteria": ["y", "y", "n"],
                    },
                }
            ),
            "'y' twice",
        ),
    ],
)
def test_what_is_not_a_question_is_refused_with_why(zerank, text, reason):
    with pytest.raises(ValueError, match=reason):
        zerank.parse_question(text)


def test_a_score_that_is_not_a_number_is_the_models_failure_not_an_answer(zerank):
    question = {"type": "noul", "instructions": "i"}
    with pytest.raises(RuntimeError, match="no usable score"):
        zerank.judge(Scores({"s": float("nan")}), *zerank.pairs_for("s", {"q": question}))


def test_a_pair_past_the_trained_positions_is_refused(zerank):
    zerank.refuse_overlong([10, 40960], 40960)
    with pytest.raises(ValueError, match="past the 40960"):
        zerank.refuse_overlong([10, 40961], 40960)


def test_blank_instructions_are_refused(zerank):
    with pytest.raises(ValueError, match="no instructions"):
        zerank.parse_question(
            json.dumps({"state": "s", "question": {"type": "noul", "instructions": "  "}})
        )
