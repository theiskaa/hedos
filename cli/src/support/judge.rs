//! Composing a typed question for a judge, and laying out the judgment it
//! answers with.
//!
//! A judge takes no prose. It takes a state and one or more typed questions, and
//! answers each with a probability distribution rather than a sentence. So the
//! composer asks for those parts in turn instead of offering a prompt box, and
//! the renderer lays the distribution out instead of printing the envelope.
//!
//! The request is written by hand rather than through a map type, because the
//! order of a choice's options is part of the question: it changes the
//! probabilities the model gives them, and every map here sorts its keys.

use kernel::records::{Capability, ModelRecord};

use crate::error::CliError;
use crate::support::interactive;
use crate::support::output::Out;

/// How wide a probability bar may draw, in cells.
const BAR_CELLS: usize = 24;

/// What a judge can be asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Pick one of several labelled options.
    Choice,
    /// Rate against ordered levels.
    Score,
    /// How far a statement holds.
    Noul,
}

impl Kind {
    /// The wire name.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Choice => "choice",
            Self::Score => "score",
            Self::Noul => "noul",
        }
    }

    /// The line offered in the picker.
    fn description(self) -> &'static str {
        match self {
            Self::Choice => "choice  ·  pick one of several options",
            Self::Score => "score   ·  rate against ordered levels",
            Self::Noul => "noul    ·  how far a statement holds",
        }
    }
}

/// One typed question: what is asked, and what it may be answered with.
#[derive(Debug, Clone)]
pub(crate) struct Question {
    /// The key the answer comes back under.
    pub(crate) id: String,
    /// Which kind of answer is wanted.
    pub(crate) kind: Kind,
    /// The question itself.
    pub(crate) instructions: String,
    /// A choice's options as `(label, description)` in the order given, or a
    /// score's levels as `(level text, "")`. Empty for a noul.
    pub(crate) criteria: Vec<(String, String)>,
}

/// Whether `record` answers typed questions.
pub(crate) fn is_judge(record: &ModelRecord) -> bool {
    record.capabilities.contains(&Capability::judge())
}

/// Ask for the situation being judged and the questions to put to it. The
/// caller must have confirmed the session is interactive.
pub(crate) fn compose(out: &Out, model: &str) -> Result<(String, Vec<Question>), CliError> {
    out.err(&format!("{model} answers typed questions, not prose."));
    out.err("  situation: what is being judged (a ticket, a log line, a message).");
    out.err("             press enter to skip it when the question stands on its own.");
    out.err("  question:  what you want decided about it, and what it may answer with.");
    // Empty is a real answer here. "which is darker, white or black" carries its
    // whole world in the question, and inventing a situation for it only adds
    // noise for the model to weigh.
    let state = interactive::input("situation", true)?;
    let mut questions = Vec::new();
    loop {
        let index = questions.len() + 1;
        questions.push(ask_question(index)?);
        if !interactive::confirm("another question about the same state?", false)? {
            break;
        }
    }
    Ok((state, questions))
}

fn ask_question(index: usize) -> Result<Question, CliError> {
    let kinds = [Kind::Choice, Kind::Score, Kind::Noul];
    let labels: Vec<String> = kinds
        .iter()
        .map(|kind| kind.description().to_owned())
        .collect();
    let kind = kinds
        .get(interactive::select_index("answer with", &labels)?)
        .copied()
        .ok_or_else(|| CliError::new("nothing selected"))?;
    let instructions = interactive::input("question", false)?;
    let criteria = match kind {
        Kind::Choice => collect_criteria("option", "`label: what it means`, or just a label")?,
        Kind::Score => collect_criteria("level", "lowest level first")?,
        Kind::Noul => Vec::new(),
    };
    Ok(Question {
        id: format!("q{index}"),
        kind,
        instructions,
        criteria,
    })
}

/// Read a numbered list of options or levels, ending on an empty line. Two is
/// the floor: a judge weighs alternatives against each other, and one of them
/// carries the whole distribution whatever it says.
fn collect_criteria(noun: &str, hint: &str) -> Result<Vec<(String, String)>, CliError> {
    let mut entries: Vec<(String, String)> = Vec::new();
    loop {
        let line = interactive::input(&format!("{noun} {}", entries.len() + 1), true)?;
        let line = line.trim();
        if line.is_empty() {
            if entries.len() >= 2 {
                return Ok(entries);
            }
            return Err(CliError::new(format!(
                "a {noun} list needs at least two entries — {hint}"
            )));
        }
        let (label, description) = match line.split_once(':') {
            Some((label, description)) => (label.trim(), description.trim()),
            None => (line, ""),
        };
        if label.is_empty() {
            return Err(CliError::new(format!("a {noun} needs a label")));
        }
        if entries.iter().any(|(existing, _)| existing == label) {
            return Err(CliError::new(format!(
                "{noun} \"{label}\" was already given — each one is keyed by its label"
            )));
        }
        entries.push((label.to_owned(), description.to_owned()));
    }
}

/// The request text a judge reads: the state and the questions, with every
/// option left exactly where it was put.
pub(crate) fn request_text(state: &str, questions: &[Question]) -> String {
    let mut text = String::from("{\"state\":");
    text.push_str(&quoted(state));
    text.push_str(",\"questions\":{");
    for (index, question) in questions.iter().enumerate() {
        if index > 0 {
            text.push(',');
        }
        text.push_str(&quoted(&question.id));
        text.push_str(":{\"type\":");
        text.push_str(&quoted(question.kind.as_str()));
        text.push_str(",\"instructions\":");
        text.push_str(&quoted(&question.instructions));
        match question.kind {
            Kind::Choice => {
                text.push_str(",\"criteria\":{");
                for (position, (label, description)) in question.criteria.iter().enumerate() {
                    if position > 0 {
                        text.push(',');
                    }
                    text.push_str(&quoted(label));
                    text.push(':');
                    text.push_str(&quoted(description));
                }
                text.push('}');
            }
            Kind::Score => {
                text.push_str(",\"criteria\":[");
                for (position, (label, _)) in question.criteria.iter().enumerate() {
                    if position > 0 {
                        text.push(',');
                    }
                    text.push_str(&quoted(label));
                }
                text.push(']');
            }
            Kind::Noul => {}
        }
        text.push('}');
    }
    text.push_str("}}");
    text
}

/// Recover the questions from a request written by hand, so a judgment answered
/// to one reads the same as a composed one. A choice's options come back in
/// whatever order the parser hands over, which the layout does not rest on: it
/// ranks by probability and looks each option up by its label. A score's levels
/// are a list, so their order, which is their meaning, survives.
pub(crate) fn parse_questions(text: &str) -> Vec<Question> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(questions) = value
        .get("questions")
        .and_then(serde_json::Value::as_object)
    else {
        return Vec::new();
    };
    let mut recovered: Vec<Question> = questions
        .iter()
        .filter_map(|(id, question)| {
            let kind = match question.get("type").and_then(serde_json::Value::as_str)? {
                "choice" => Kind::Choice,
                "score" => Kind::Score,
                "noul" => Kind::Noul,
                _ => return None,
            };
            Some(Question {
                id: id.clone(),
                kind,
                instructions: plain(question.get("instructions")),
                criteria: match question.get("criteria") {
                    Some(serde_json::Value::Object(options)) => options
                        .iter()
                        .map(|(label, description)| (label.clone(), plain(Some(description))))
                        .collect(),
                    Some(serde_json::Value::Array(levels)) => levels
                        .iter()
                        .map(|level| (plain(Some(level)), String::new()))
                        .collect(),
                    _ => Vec::new(),
                },
            })
        })
        .collect();
    // A map sorts its keys, so the questions come back in an order nobody chose.
    // Put them back in the order they were written, which is the order they
    // will be read in.
    recovered.sort_by_key(|question| {
        text.find(&format!("\"{}\"", question.id))
            .unwrap_or(usize::MAX)
    });
    recovered
}

/// A JSON value as the text to show for it: a string as itself, anything else
/// as the JSON it is, since a question may carry structure rather than prose.
fn plain(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// A JSON string literal for `value`. Encoding a string cannot fail; the empty
/// literal stands in rather than a panic if it ever did.
fn quoted(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_owned())
}

/// Lay out the envelope `reply` against the `questions` that produced it.
pub(crate) fn render(out: &Out, reply: &str, questions: &[Question]) -> Result<(), CliError> {
    let envelope: serde_json::Value = serde_json::from_str(reply)
        .map_err(|_| CliError::new(format!("the model did not answer with a judgment: {reply}")))?;
    let answers = envelope
        .get("answers")
        .ok_or_else(|| CliError::new(format!("the judgment carried no answers: {reply}")))?;
    for question in questions {
        let Some(answer) = answers.get(&question.id) else {
            out.err(&format!("{} went unanswered", question.instructions));
            continue;
        };
        out.line("");
        out.line(&question.instructions);
        for line in lines_for(question, answer) {
            out.line(&line);
        }
    }
    Ok(())
}

/// The laid-out lines for one answer, by what was asked.
fn lines_for(question: &Question, answer: &serde_json::Value) -> Vec<String> {
    let confidence = answer.get("confidence").and_then(serde_json::Value::as_f64);
    let mut lines = match question.kind {
        Kind::Choice => choice_lines(question, answer),
        Kind::Score => score_lines(question, answer),
        Kind::Noul => noul_lines(answer),
    };
    if let Some(confidence) = confidence {
        lines.push(format!("  confidence {confidence:.2}"));
    }
    lines
}

fn choice_lines(question: &Question, answer: &serde_json::Value) -> Vec<String> {
    let chosen = answer.get("choice").and_then(serde_json::Value::as_str);
    let probabilities = answer.get("probabilities");
    let mut rows: Vec<(&str, &str, f64)> = question
        .criteria
        .iter()
        .map(|(label, description)| {
            let probability = probabilities
                .and_then(|map| map.get(label))
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0);
            (label.as_str(), description.as_str(), probability)
        })
        .collect();
    // Ranked for reading. The order they were asked in still decided the
    // numbers; it just isn't the order they are most useful in.
    rows.sort_by(|left, right| right.2.total_cmp(&left.2));
    let widest = rows
        .iter()
        .map(|(label, ..)| label.len())
        .max()
        .unwrap_or(0);
    rows.iter()
        .map(|(label, description, probability)| {
            let marker = if Some(*label) == chosen { "▸" } else { " " };
            let detail = if description.is_empty() {
                String::new()
            } else {
                format!("  {description}")
            };
            format!(
                "  {marker} {label:<widest$}  {}  {:>5}{detail}",
                bar(*probability),
                percent(*probability),
            )
        })
        .collect()
}

fn score_lines(question: &Question, answer: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(score) = answer.get("score").and_then(serde_json::Value::as_f64) {
        let last = question.criteria.len().saturating_sub(1);
        lines.push(format!("  score {score:.2} of 0–{last}"));
    }
    let probabilities = answer.get("probabilities");
    let widest = question
        .criteria
        .iter()
        .map(|(label, _)| label.len())
        .max()
        .unwrap_or(0);
    for (level, (label, _)) in question.criteria.iter().enumerate() {
        let probability = probabilities
            .and_then(|map| map.get(level.to_string()))
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        lines.push(format!(
            "    {level} {label:<widest$}  {}  {:>5}",
            bar(probability),
            percent(probability),
        ));
    }
    lines
}

fn noul_lines(answer: &serde_json::Value) -> Vec<String> {
    let holds = answer
        .get("noul")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    vec![
        format!("    yes  {}  {:>5}", bar(holds), percent(holds)),
        format!("    no   {}  {:>5}", bar(1.0 - holds), percent(1.0 - holds)),
    ]
}

/// A bar `fraction` of the way across [`BAR_CELLS`]. Anything above nothing at
/// all draws at least a sliver, so a small probability reads as small rather
/// than as absent.
fn bar(fraction: f64) -> String {
    let clamped = fraction.clamp(0.0, 1.0);
    let filled = (clamped * BAR_CELLS as f64).round() as usize;
    if filled == 0 && clamped > 0.0 {
        return format!("▏{}", " ".repeat(BAR_CELLS - 1));
    }
    format!("{}{}", "█".repeat(filled), " ".repeat(BAR_CELLS - filled))
}

fn percent(fraction: f64) -> String {
    format!("{:.1}%", fraction * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice() -> Question {
        Question {
            id: "q1".to_owned(),
            kind: Kind::Choice,
            instructions: "Which team?".to_owned(),
            criteria: vec![
                ("zeta".to_owned(), "last letter".to_owned()),
                ("alpha".to_owned(), "first letter".to_owned()),
                ("mid".to_owned(), String::new()),
            ],
        }
    }

    #[test]
    fn a_choices_options_are_written_in_the_order_they_were_given() {
        // Alphabetical would be alpha, mid, zeta. The order asked in is the
        // order sent, because it is part of the question.
        assert_eq!(
            request_text("500s", &[choice()]),
            r#"{"state":"500s","questions":{"q1":{"type":"choice","instructions":"Which team?","criteria":{"zeta":"last letter","alpha":"first letter","mid":""}}}}"#
        );
    }

    #[test]
    fn a_score_carries_its_levels_as_a_list_and_a_noul_carries_none() {
        let score = Question {
            id: "q1".to_owned(),
            kind: Kind::Score,
            instructions: "How bad?".to_owned(),
            criteria: vec![
                ("mild".to_owned(), String::new()),
                ("dire".to_owned(), String::new()),
            ],
        };
        assert_eq!(
            request_text("s", &[score]),
            r#"{"state":"s","questions":{"q1":{"type":"score","instructions":"How bad?","criteria":["mild","dire"]}}}"#
        );
        let noul = Question {
            id: "q1".to_owned(),
            kind: Kind::Noul,
            instructions: "Holds?".to_owned(),
            criteria: Vec::new(),
        };
        assert_eq!(
            request_text("s", &[noul]),
            r#"{"state":"s","questions":{"q1":{"type":"noul","instructions":"Holds?"}}}"#
        );
    }

    #[test]
    fn a_state_with_quotes_and_newlines_stays_one_json_string() {
        let text = request_text("he said \"no\"\nthen left", &[choice()]);
        assert!(
            text.contains(r#""state":"he said \"no\"\nthen left""#),
            "{text}"
        );
        assert!(serde_json::from_str::<serde_json::Value>(&text).is_ok());
    }

    #[test]
    fn several_questions_keep_their_order_too() {
        let mut second = choice();
        second.id = "q2".to_owned();
        let text = request_text("s", &[choice(), second]);
        let first_at = text.find("\"q1\"").expect("q1");
        let second_at = text.find("\"q2\"").expect("q2");
        assert!(first_at < second_at, "{text}");
    }

    #[test]
    fn a_hand_written_request_is_recovered_well_enough_to_lay_out() {
        let recovered = parse_questions(&request_text("s", &[choice()]));
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].kind, Kind::Choice);
        assert_eq!(recovered[0].instructions, "Which team?");
        // Sorted by the parser, which the layout does not depend on.
        let labels: Vec<&str> = recovered[0]
            .criteria
            .iter()
            .map(|(label, _)| label.as_str())
            .collect();
        assert_eq!(labels, ["alpha", "mid", "zeta"]);

        // A score's levels are a list, so their order is kept as given.
        let score = Question {
            id: "q1".to_owned(),
            kind: Kind::Score,
            instructions: "How bad?".to_owned(),
            criteria: vec![
                ("mild".to_owned(), String::new()),
                ("dire".to_owned(), String::new()),
            ],
        };
        let recovered = parse_questions(&request_text("s", &[score]));
        let levels: Vec<&str> = recovered[0]
            .criteria
            .iter()
            .map(|(label, _)| label.as_str())
            .collect();
        assert_eq!(levels, ["mild", "dire"]);
    }

    #[test]
    fn questions_are_recovered_in_the_order_they_were_written() {
        // Alphabetically "outage" precedes "sev", which is not how it was asked.
        let text = r#"{"state":"s","questions":{"sev":{"type":"score","instructions":"How bad?","criteria":["mild","dire"]},"outage":{"type":"noul","instructions":"Down?"}}}"#;
        let ids: Vec<String> = parse_questions(text)
            .into_iter()
            .map(|question| question.id)
            .collect();
        assert_eq!(ids, ["sev", "outage"]);
    }

    #[test]
    fn anything_that_is_not_a_request_recovers_nothing() {
        assert!(parse_questions("which color is darker").is_empty());
        assert!(parse_questions(r#"{"state":"s"}"#).is_empty());
        assert!(parse_questions(r#"{"questions":{"q":{"type":"essay"}}}"#).is_empty());
    }

    #[test]
    fn a_choice_is_laid_out_ranked_with_the_winner_marked() {
        let answer = serde_json::json!({
            "choice": "alpha",
            "probabilities": {"zeta": 0.1, "alpha": 0.8, "mid": 0.1},
            "confidence": 0.65,
        });
        let lines = lines_for(&choice(), &answer);
        assert!(lines[0].contains("▸ alpha"), "{lines:?}");
        assert!(lines[0].contains("80.0%"), "{lines:?}");
        // The two tied also-rans follow, and the confidence closes it.
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[3], "  confidence 0.65");
        // A described option carries its description, a bare one does not.
        assert!(lines[0].contains("first letter"), "{lines:?}");
        assert!(lines.iter().any(|line| line.trim_end().ends_with("10.0%")));
    }

    #[test]
    fn a_noul_reads_as_yes_against_no() {
        let answer = serde_json::json!({ "noul": 0.25 });
        let question = Question {
            id: "q1".to_owned(),
            kind: Kind::Noul,
            instructions: "Holds?".to_owned(),
            criteria: Vec::new(),
        };
        let lines = lines_for(&question, &answer);
        assert!(
            lines[0].contains("yes") && lines[0].contains("25.0%"),
            "{lines:?}"
        );
        assert!(
            lines[1].contains("no") && lines[1].contains("75.0%"),
            "{lines:?}"
        );
    }

    #[test]
    fn a_bar_shows_a_sliver_rather_than_nothing_for_a_small_share() {
        assert!(bar(0.0).trim().is_empty());
        assert_eq!(bar(0.001).trim(), "▏");
        assert_eq!(bar(1.0).trim().chars().count(), BAR_CELLS);
        // Every bar occupies the same width, so the columns after it line up.
        for fraction in [0.0, 0.001, 0.5, 1.0] {
            assert_eq!(bar(fraction).chars().count(), BAR_CELLS);
        }
    }
}
