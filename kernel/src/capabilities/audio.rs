//! A request to a model that makes audio, checked against the parameters that
//! model takes before it becomes a job. Every surface (the gateway's
//! `/v1/audio/generations`, `hedos audio`) builds its request here, so a key a
//! model has no use for is refused the same way everywhere instead of being
//! dropped by whichever runtime receives it.

use std::collections::BTreeMap;

use crate::records::{JsonValue, ParamSpec, ParamType};

/// The file formats a finished piece comes in, the first the default.
pub const AUDIO_FORMATS: [&str; 2] = ["flac", "wav"];

/// Keys any audio request may carry, whatever the model declares: what to
/// make, the file to make it as, and a request for no vocals, which a model
/// that never sings already satisfies.
const ALWAYS_TAKEN: [&str; 3] = ["prompt", "format", "instrumental"];

/// Why an audio request was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AudioRequestError {
    /// No description of what to make.
    #[error("a prompt is required: describe what to make")]
    MissingPrompt,
    /// A format other than [`AUDIO_FORMATS`].
    #[error("audio comes as flac or wav, not {0}")]
    Format(String),
    /// A key the model takes no parameter for.
    #[error("{model} does not take {key}; it takes {takes}")]
    NotTaken {
        /// The model's name.
        model: String,
        /// The key it does not take.
        key: String,
        /// What it takes, written out.
        takes: String,
    },
    /// A value of the wrong kind.
    #[error("{key} should be {expected}")]
    WrongType {
        /// The key.
        key: String,
        /// What it should be (`a whole number`).
        expected: &'static str,
    },
    /// A number outside the parameter's range.
    #[error("{key} should be between {low} and {high}")]
    OutOfRange {
        /// The key.
        key: String,
        /// The lowest value it takes.
        low: String,
        /// The highest value it takes.
        high: String,
    },
    /// Lyrics alongside a request for no vocals.
    #[error("instrumental asks for no vocals, so send no lyrics with it")]
    LyricsAndInstrumental,
    /// An enum value outside its choices.
    #[error("{key} is one of {choices}")]
    NotAChoice {
        /// The key.
        key: String,
        /// Its choices, written out.
        choices: String,
    },
}

/// The job payload for `fields`, a request to the model `model` whose
/// parameter schema is `params`: the prompt, the format (`flac` unless asked
/// otherwise), and each other field checked against the parameter of its
/// name. A null field is treated as absent. Fails on the first field the model
/// does not take or whose value does not fit.
pub fn audio_payload(
    model: &str,
    fields: &BTreeMap<String, JsonValue>,
    params: &[ParamSpec],
) -> Result<JsonValue, AudioRequestError> {
    let prompt = fields
        .get("prompt")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
        .ok_or(AudioRequestError::MissingPrompt)?;
    let format = match fields.get("format") {
        None | Some(JsonValue::Null) => AUDIO_FORMATS[0].to_owned(),
        Some(JsonValue::String(format)) => {
            let lower = format.to_ascii_lowercase();
            if !AUDIO_FORMATS.contains(&lower.as_str()) {
                return Err(AudioRequestError::Format(format.clone()));
            }
            lower
        }
        Some(other) => return Err(AudioRequestError::Format(format!("{other:?}"))),
    };

    let instrumental = matches!(fields.get("instrumental"), Some(JsonValue::Bool(true)));
    let sung = fields
        .get("lyrics")
        .and_then(JsonValue::as_str)
        .is_some_and(|lyrics| !lyrics.trim().is_empty());
    if instrumental && sung {
        return Err(AudioRequestError::LyricsAndInstrumental);
    }

    let mut payload = BTreeMap::new();
    payload.insert("prompt".to_owned(), JsonValue::String(prompt.to_owned()));
    payload.insert("format".to_owned(), JsonValue::String(format));
    for (key, value) in fields {
        if matches!(value, JsonValue::Null) || key == "prompt" || key == "format" {
            continue;
        }
        if key == "instrumental" {
            let JsonValue::Bool(instrumental) = value else {
                return Err(AudioRequestError::WrongType {
                    key: key.clone(),
                    expected: "true or false",
                });
            };
            payload.insert(key.clone(), JsonValue::Bool(*instrumental));
            continue;
        }
        let Some(spec) = params.iter().find(|spec| spec.key == *key) else {
            return Err(AudioRequestError::NotTaken {
                model: model.to_owned(),
                key: key.clone(),
                takes: takes(params),
            });
        };
        payload.insert(key.clone(), checked(spec, value)?);
    }
    Ok(JsonValue::Object(payload))
}

/// What a model with `params` takes, as a sentence fragment.
fn takes(params: &[ParamSpec]) -> String {
    let mut keys: Vec<&str> = ALWAYS_TAKEN.to_vec();
    keys.extend(
        params
            .iter()
            .map(|spec| spec.key.as_str())
            .filter(|key| !ALWAYS_TAKEN.contains(key)),
    );
    keys.join(", ")
}

/// `value` as parameter `spec` takes it: a float accepts a whole number, and a
/// number must sit inside the declared range.
fn checked(spec: &ParamSpec, value: &JsonValue) -> Result<JsonValue, AudioRequestError> {
    let wrong = |expected| AudioRequestError::WrongType {
        key: spec.key.clone(),
        expected,
    };
    let value = match (spec.param_type, value) {
        (ParamType::Int, JsonValue::Int(number)) => JsonValue::Int(*number),
        (ParamType::Int, _) => return Err(wrong("a whole number")),
        (ParamType::Float, JsonValue::Int(number)) => JsonValue::Double(*number as f64),
        (ParamType::Float, JsonValue::Double(number)) if number.is_finite() => {
            JsonValue::Double(*number)
        }
        (ParamType::Float, _) => return Err(wrong("a number")),
        (ParamType::Bool, JsonValue::Bool(flag)) => JsonValue::Bool(*flag),
        (ParamType::Bool, _) => return Err(wrong("true or false")),
        (ParamType::String, JsonValue::String(text)) => JsonValue::String(text.clone()),
        (ParamType::String, _) => return Err(wrong("text")),
        (ParamType::Enum, JsonValue::String(choice)) => {
            let choices = spec.values.as_deref().unwrap_or_default();
            if !choices.contains(choice) {
                return Err(AudioRequestError::NotAChoice {
                    key: spec.key.clone(),
                    choices: choices.join(", "),
                });
            }
            JsonValue::String(choice.clone())
        }
        (ParamType::Enum, _) => return Err(wrong("text")),
    };
    if let Some([low, high]) = spec.range.as_deref()
        && !within(&value, low, high)
    {
        return Err(AudioRequestError::OutOfRange {
            key: spec.key.clone(),
            low: written(low),
            high: written(high),
        });
    }
    Ok(value)
}

/// Whether number `value` lies in `[low, high]`: whole numbers compared
/// exactly, since past 2^53 two of them can read as the same float.
fn within(value: &JsonValue, low: &JsonValue, high: &JsonValue) -> bool {
    match (value, low, high) {
        (JsonValue::Int(value), JsonValue::Int(low), JsonValue::Int(high)) => {
            (low..=high).contains(&value)
        }
        _ => match (value.as_f64(), low.as_f64(), high.as_f64()) {
            (Some(value), Some(low), Some(high)) => (low..=high).contains(&value),
            _ => true,
        },
    }
}

fn written(value: &JsonValue) -> String {
    match value {
        JsonValue::Int(number) => number.to_string(),
        JsonValue::Double(number) => number.to_string(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(key: &str, param_type: ParamType) -> ParamSpec {
        ParamSpec {
            key: key.to_owned(),
            param_type,
            default_value: None,
            range: None,
            values: None,
        }
    }

    fn ranged(key: &str, param_type: ParamType, low: JsonValue, high: JsonValue) -> ParamSpec {
        ParamSpec {
            range: Some(vec![low, high]),
            ..spec(key, param_type)
        }
    }

    /// Stable Audio's schema, as the pipeline table declares it.
    fn stable_audio() -> Vec<ParamSpec> {
        vec![
            ranged(
                "duration",
                ParamType::Float,
                JsonValue::Double(1.0),
                JsonValue::Double(47.0),
            ),
            ranged(
                "steps",
                ParamType::Int,
                JsonValue::Int(1),
                JsonValue::Int(500),
            ),
            spec("seed", ParamType::Int),
            spec("negative_prompt", ParamType::String),
        ]
    }

    fn fields(pairs: &[(&str, JsonValue)]) -> BTreeMap<String, JsonValue> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    fn text(value: &str) -> JsonValue {
        JsonValue::String(value.to_owned())
    }

    #[test]
    fn a_request_becomes_a_payload_with_a_format() {
        let payload = audio_payload(
            "Stable Audio",
            &fields(&[
                ("prompt", text(" rain on a tin roof ")),
                ("duration", JsonValue::Int(12)),
                ("seed", JsonValue::Int(7)),
                ("negative_prompt", JsonValue::Null),
            ]),
            &stable_audio(),
        )
        .expect("taken");
        assert_eq!(
            payload,
            JsonValue::Object(fields(&[
                ("prompt", text("rain on a tin roof")),
                ("format", text("flac")),
                ("duration", JsonValue::Double(12.0)),
                ("seed", JsonValue::Int(7)),
            ]))
        );
    }

    #[test]
    fn lyrics_to_a_model_that_cannot_sing_are_refused_naming_what_it_takes() {
        let error = audio_payload(
            "Stable Audio",
            &fields(&[("prompt", text("x")), ("lyrics", text("la la"))]),
            &stable_audio(),
        )
        .expect_err("refused");
        assert_eq!(
            error.to_string(),
            "Stable Audio does not take lyrics; it takes prompt, format, instrumental, \
             duration, steps, seed, negative_prompt"
        );
    }

    #[test]
    fn a_prompt_is_required() {
        for prompt in [None, Some(text("  ")), Some(JsonValue::Int(3))] {
            let mut request = BTreeMap::new();
            if let Some(prompt) = prompt {
                request.insert("prompt".to_owned(), prompt);
            }
            assert_eq!(
                audio_payload("m", &request, &[]),
                Err(AudioRequestError::MissingPrompt)
            );
        }
    }

    #[test]
    fn only_flac_and_wav_are_made() {
        let wav = audio_payload(
            "m",
            &fields(&[("prompt", text("x")), ("format", text("WAV"))]),
            &[],
        )
        .unwrap();
        assert_eq!(
            wav,
            JsonValue::Object(fields(&[("prompt", text("x")), ("format", text("wav"))]))
        );
        assert_eq!(
            audio_payload(
                "m",
                &fields(&[("prompt", text("x")), ("format", text("mp3"))]),
                &[]
            ),
            Err(AudioRequestError::Format("mp3".to_owned()))
        );
    }

    #[test]
    fn values_must_fit_their_parameter() {
        let cot = ParamSpec {
            values: Some(vec!["full".to_owned(), "off".to_owned()]),
            ..spec("cot", ParamType::Enum)
        };
        let mut params = stable_audio();
        params.push(cot);
        for (key, value, message) in [
            (
                "duration",
                JsonValue::Int(60),
                "duration should be between 1 and 47",
            ),
            (
                "steps",
                JsonValue::Double(2.5),
                "steps should be a whole number",
            ),
            ("seed", text("seven"), "seed should be a whole number"),
            (
                "negative_prompt",
                JsonValue::Int(1),
                "negative_prompt should be text",
            ),
            ("cot", text("some"), "cot is one of full, off"),
            (
                "instrumental",
                text("yes"),
                "instrumental should be true or false",
            ),
        ] {
            let error = audio_payload(
                "m",
                &fields(&[("prompt", text("x")), (key, value)]),
                &params,
            )
            .expect_err(key);
            assert_eq!(error.to_string(), message);
        }
    }

    #[test]
    fn whole_numbers_past_two_to_the_fifty_three_are_compared_exactly() {
        let seed = ranged(
            "seed",
            ParamType::Int,
            JsonValue::Int(0),
            JsonValue::Int(9_007_199_254_740_992),
        );
        let request = |seed: i64| fields(&[("prompt", text("x")), ("seed", JsonValue::Int(seed))]);
        assert!(
            audio_payload(
                "m",
                &request(9_007_199_254_740_992),
                std::slice::from_ref(&seed)
            )
            .is_ok()
        );
        assert!(
            audio_payload(
                "m",
                &request(9_007_199_254_740_993),
                std::slice::from_ref(&seed)
            )
            .is_err()
        );
        let wide = ranged(
            "seed",
            ParamType::Int,
            JsonValue::Int(0),
            JsonValue::Int(i64::MAX),
        );
        assert!(audio_payload("m", &request(i64::MAX), std::slice::from_ref(&wide)).is_ok());
        assert!(audio_payload("m", &request(-1), &[wide]).is_err());
    }

    #[test]
    fn lyrics_and_instrumental_together_are_refused() {
        let lyrics = spec("lyrics", ParamType::String);
        let request = |sung: &str| {
            fields(&[
                ("prompt", text("x")),
                ("lyrics", text(sung)),
                ("instrumental", JsonValue::Bool(true)),
            ])
        };
        assert_eq!(
            audio_payload("m", &request("[verse]\nla"), std::slice::from_ref(&lyrics)),
            Err(AudioRequestError::LyricsAndInstrumental)
        );
        assert!(audio_payload("m", &request("  "), std::slice::from_ref(&lyrics)).is_ok());
    }

    #[test]
    fn instrumental_is_taken_by_any_model() {
        let payload = audio_payload(
            "m",
            &fields(&[
                ("prompt", text("x")),
                ("instrumental", JsonValue::Bool(true)),
            ]),
            &[],
        )
        .expect("taken");
        assert!(
            matches!(payload, JsonValue::Object(map) if map.get("instrumental") == Some(&JsonValue::Bool(true)))
        );
    }
}
