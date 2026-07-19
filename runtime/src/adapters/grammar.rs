//! GBNF grammars for constrained decoding: a generic JSON grammar for
//! `response_format`, and a tool grammar that forces a model to emit exactly one
//! `<tool_call>{…}</tool_call>` block matching one of the offered tools' schemas.
//! The grammar strings are handed to llama.cpp (directly or via llama-server's
//! `grammar` parameter).

use std::collections::BTreeMap;
use std::collections::HashSet;

use kernel::capabilities::ToolSpec;
use kernel::records::JsonValue;

/// Why a set of tool schemas can't be turned into a grammar.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolGrammarError {
    /// No tools were offered.
    #[error("no tools were provided")]
    NoTools,
    /// A tool's schema uses something the grammar builder can't express.
    #[error("{tool}: {detail}")]
    UnsupportedSchema {
        /// The offending tool's name.
        tool: String,
        /// What went wrong.
        detail: String,
    },
}

/// The opening marker of a tool-call block.
pub const CALL_OPEN: &str = "<tool_call>";
/// The closing marker of a tool-call block.
pub const CALL_CLOSE: &str = "</tool_call>";

/// A generic JSON grammar, used when a request asks for JSON output.
pub const GENERIC_JSON_GRAMMAR: &str = r#"root ::= object
value ::= object | array | string | number | ("true" | "false" | "null") ws
object ::= "{" ws ( string ":" ws value ("," ws string ":" ws value)* )? "}" ws
array ::= "[" ws ( value ("," ws value)* )? "]" ws
string ::= "\"" ( [^"\\] | "\\" (["\\/bfnrt] | "u" [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F]) )* "\"" ws
number ::= ("-"? ([0-9] | [1-9] [0-9]*)) ("." [0-9]+)? ([eE] [-+]? [0-9]+)? ws
ws ::= [ \t\n]*"#;

/// The generic JSON grammar if `value` is a `response_format` object with a
/// `type`, else `None` (free-form output).
pub fn grammar_for_response_format(value: Option<&JsonValue>) -> Option<&'static str> {
    match value {
        Some(JsonValue::Object(fields))
            if fields.get("type").and_then(JsonValue::as_str).is_some() =>
        {
            Some(GENERIC_JSON_GRAMMAR)
        }
        _ => None,
    }
}

fn unsupported(tool: &str, detail: &str) -> ToolGrammarError {
    ToolGrammarError::UnsupportedSchema {
        tool: tool.to_owned(),
        detail: detail.to_owned(),
    }
}

/// Whether `c` is safe to embed literally in a GBNF string (plain ASCII, no
/// newline, quote, or backslash).
fn is_plain_ascii(c: char) -> bool {
    c.is_ascii() && !matches!(c, '\n' | '\r' | '\u{0B}' | '\u{0C}') && c != '"' && c != '\\'
}

/// Escape a string for embedding in a GBNF double-quoted literal.
fn escaped(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The GBNF grammar constraining a model to emit one valid tool call.
pub fn tool_grammar(tools: &[ToolSpec]) -> Result<String, ToolGrammarError> {
    if tools.is_empty() {
        return Err(ToolGrammarError::NoTools);
    }
    let mut rules: Vec<String> = Vec::new();
    let mut call_rules: Vec<String> = Vec::new();
    for (index, tool) in tools.iter().enumerate() {
        if !tool.name.chars().all(is_plain_ascii) {
            return Err(unsupported(&tool.name, "tool names must be plain ASCII"));
        }
        let rule_name = format!("call-{index}");
        let args_rule = format!("args-{index}");
        call_rules.push(rule_name.clone());
        rules.push(format!(
            r#"{rule_name} ::= "{{" space "\"name\"" space ":" space "\"{name}\"" space "," space "\"arguments\"" space ":" space {args_rule} space "}}""#,
            name = tool.name,
        ));
        rules.extend(value_rules(&args_rule, &tool.parameters, &tool.name)?);
    }
    let mut grammar = format!(
        r#"root ::= "{open}" space call space "{close}"
call ::= {calls}
space ::= [ \t\n\r]*
string ::= "\"" ([^"\\] | "\\" .)* "\""
number ::= "-"? [0-9]+ ("." [0-9]+)? ([eE] [-+]? [0-9]+)?
integer ::= "-"? [0-9]+
boolean ::= "true" | "false"

"#,
        open = CALL_OPEN,
        close = CALL_CLOSE,
        calls = call_rules.join(" | "),
    );
    grammar.push_str(&rules.join("\n"));
    grammar.push('\n');
    Ok(grammar)
}

/// The grammar rules for one schema node, named `name`.
fn value_rules(
    name: &str,
    schema: &JsonValue,
    tool: &str,
) -> Result<Vec<String>, ToolGrammarError> {
    let JsonValue::Object(fields) = schema else {
        return Err(unsupported(tool, "schema must be an object"));
    };

    if let Some(JsonValue::Array(options)) = fields.get("enum") {
        let mut literals = Vec::with_capacity(options.len());
        for option in options {
            let text = option
                .as_str()
                .ok_or_else(|| unsupported(tool, "enum values must be strings"))?;
            literals.push(format!(r#""\"{}\"""#, escaped(text)));
        }
        return Ok(vec![format!("{name} ::= {}", literals.join(" | "))]);
    }

    let node_type = fields
        .get("type")
        .and_then(JsonValue::as_str)
        .unwrap_or("object");
    match node_type {
        "string" => Ok(vec![format!("{name} ::= string")]),
        "number" => Ok(vec![format!("{name} ::= number")]),
        "integer" => Ok(vec![format!("{name} ::= integer")]),
        "boolean" => Ok(vec![format!("{name} ::= boolean")]),
        "array" => {
            let items = fields
                .get("items")
                .ok_or_else(|| unsupported(tool, "arrays need an items schema"))?;
            let item_rule = format!("{name}-item");
            let mut rules = vec![format!(
                r#"{name} ::= "[" space ({item_rule} (space "," space {item_rule})*)? space "]""#
            )];
            rules.extend(value_rules(&item_rule, items, tool)?);
            Ok(rules)
        }
        "object" => object_rules(name, fields, tool),
        other => Err(unsupported(tool, &format!("unsupported type {other}"))),
    }
}

/// The grammar rules for an object schema, with required/optional property
/// ordering matching the Swift builder.
fn object_rules(
    name: &str,
    fields: &BTreeMap<String, JsonValue>,
    tool: &str,
) -> Result<Vec<String>, ToolGrammarError> {
    let empty = BTreeMap::new();
    let properties = match fields.get("properties") {
        None => &empty,
        Some(JsonValue::Object(props)) => props,
        Some(_) => return Err(unsupported(tool, "properties must be an object")),
    };

    let mut required: HashSet<&str> = HashSet::new();
    if let Some(JsonValue::Array(names)) = fields.get("required") {
        for entry in names {
            let text = entry
                .as_str()
                .ok_or_else(|| unsupported(tool, "required entries must be strings"))?;
            required.insert(text);
        }
    }

    if properties.is_empty() {
        return Ok(vec![format!(r#"{name} ::= "{{" space "}}""#)]);
    }
    for key in properties.keys() {
        if !key.chars().all(is_plain_ascii) {
            return Err(unsupported(tool, "property names must be plain ASCII"));
        }
    }

    // `properties` is a BTreeMap, so iteration is already key-sorted (Swift sorts).
    let mut rules: Vec<String> = Vec::new();
    let mut pairs: Vec<(&str, String)> = Vec::new();
    for (position, (key, schema)) in properties.iter().enumerate() {
        let value_rule = format!("{name}-p{position}");
        pairs.push((
            key.as_str(),
            format!(r#""\"{}\"" space ":" space {value_rule}"#, escaped(key)),
        ));
        rules.extend(value_rules(&value_rule, schema, tool)?);
    }

    let required_pairs: Vec<&(&str, String)> = pairs
        .iter()
        .filter(|(key, _)| required.contains(key))
        .collect();
    let optional_pairs: Vec<&(&str, String)> = pairs
        .iter()
        .filter(|(key, _)| !required.contains(key))
        .collect();

    let body = if required_pairs.is_empty() {
        let branches: Vec<String> = optional_pairs
            .iter()
            .enumerate()
            .map(|(start, (_, pair))| {
                let mut branch = pair.clone();
                for (_, later) in optional_pairs.iter().skip(start + 1) {
                    branch.push_str(&format!(r#" (space "," space {later})?"#));
                }
                branch
            })
            .collect();
        format!("( {} )?", branches.join(" | "))
    } else {
        let mut segments: Vec<String> = Vec::new();
        for (index, (_, pair)) in required_pairs.iter().enumerate() {
            segments.push(if index == 0 {
                pair.clone()
            } else {
                format!(r#"space "," space {pair}"#)
            });
        }
        for (_, pair) in &optional_pairs {
            segments.push(format!(r#"(space "," space {pair})?"#));
        }
        segments.join(" ")
    };

    rules.insert(0, format!(r#"{name} ::= "{{" space {body} space "}}""#));
    Ok(rules)
}

/// The system-prompt block describing the offered tools and the call format, for
/// a model with no native tool support.
pub fn tool_system_block(tools: &[ToolSpec]) -> String {
    let mut lines = vec![
        "You can call tools. The available tools are:".to_owned(),
        String::new(),
    ];
    for tool in tools {
        lines.push(format!(
            "- {}: {} Parameters schema: {}",
            tool.name,
            tool.description,
            json_string(&tool.parameters),
        ));
    }
    lines.push(String::new());
    lines.push(format!(
        "To call a tool, reply with exactly one block of the form {open}{{\"name\": \"<tool name>\", \"arguments\": {{…}}}}{close} and nothing after it. Only call a tool when it is needed to answer.",
        open = CALL_OPEN,
        close = CALL_CLOSE,
    ));
    lines.join("\n")
}

fn json_string(value: &JsonValue) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(pairs: &[(&str, JsonValue)]) -> JsonValue {
        JsonValue::Object(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    }

    fn string(value: &str) -> JsonValue {
        JsonValue::String(value.to_owned())
    }

    fn spec(name: &str, parameters: JsonValue) -> ToolSpec {
        ToolSpec::new(name, "", parameters)
    }

    #[test]
    fn no_tools_is_an_error() {
        assert_eq!(tool_grammar(&[]).unwrap_err(), ToolGrammarError::NoTools);
    }

    #[test]
    fn a_non_ascii_tool_name_is_rejected() {
        let tools = [spec("weather☂", object(&[("type", string("object"))]))];
        assert!(matches!(
            tool_grammar(&tools),
            Err(ToolGrammarError::UnsupportedSchema { .. })
        ));
    }

    #[test]
    fn an_object_schema_orders_required_then_optional() {
        let params = object(&[
            ("type", string("object")),
            (
                "properties",
                object(&[
                    ("a", object(&[("type", string("integer"))])),
                    ("b", object(&[("type", string("string"))])),
                ]),
            ),
            ("required", JsonValue::Array(vec![string("a")])),
        ]);
        let grammar = tool_grammar(&[spec("add", params)]).unwrap();
        // The root/call header and the named literal are present.
        assert!(grammar.contains(r#"root ::= "<tool_call>" space call space "</tool_call>""#));
        assert!(grammar.contains(r#""\"name\"" space ":" space "\"add\"""#));
        // `a` is required, `b` optional: the required pair leads, the optional is
        // wrapped in `( … )?`.
        assert!(grammar.contains("args-0-p0")); // a
        assert!(grammar.contains(r#"(space "," space "\"b\"""#));
        assert!(grammar.contains("args-0-p1 ::= string")); // b
        assert!(grammar.contains("args-0-p0 ::= integer")); // a
    }

    #[test]
    fn all_optional_properties_use_the_branch_form() {
        let params = object(&[
            ("type", string("object")),
            (
                "properties",
                object(&[("x", object(&[("type", string("boolean"))]))]),
            ),
        ]);
        let grammar = tool_grammar(&[spec("t", params)]).unwrap();
        assert!(grammar.contains("( ")); // the `( … )?` optional-branch wrapper
        assert!(grammar.contains("args-0-p0 ::= boolean"));
    }

    #[test]
    fn an_enum_becomes_a_literal_alternation() {
        let params = object(&[
            ("type", string("object")),
            (
                "properties",
                object(&[(
                    "unit",
                    object(&[("enum", JsonValue::Array(vec![string("c"), string("f")]))]),
                )]),
            ),
            ("required", JsonValue::Array(vec![string("unit")])),
        ]);
        let grammar = tool_grammar(&[spec("weather", params)]).unwrap();
        assert!(grammar.contains(r#"args-0-p0 ::= "\"c\"" | "\"f\"""#));
    }

    #[test]
    fn an_array_schema_recurses_into_its_items() {
        let params = object(&[
            ("type", string("object")),
            (
                "properties",
                object(&[(
                    "tags",
                    object(&[
                        ("type", string("array")),
                        ("items", object(&[("type", string("string"))])),
                    ]),
                )]),
            ),
            ("required", JsonValue::Array(vec![string("tags")])),
        ]);
        let grammar = tool_grammar(&[spec("t", params)]).unwrap();
        assert!(grammar.contains(r#"args-0-p0 ::= "[" space"#));
        assert!(grammar.contains("args-0-p0-item ::= string"));
    }

    #[test]
    fn an_empty_object_schema_is_the_empty_braces_rule() {
        let params = object(&[("type", string("object"))]);
        let grammar = tool_grammar(&[spec("noop", params)]).unwrap();
        assert!(grammar.contains(r#"args-0 ::= "{" space "}""#));
    }

    #[test]
    fn an_unsupported_type_is_rejected() {
        let params = object(&[
            ("type", string("object")),
            (
                "properties",
                object(&[("x", object(&[("type", string("date"))]))]),
            ),
        ]);
        assert!(tool_grammar(&[spec("t", params)]).is_err());
    }

    #[test]
    fn response_format_grammar_needs_a_typed_object() {
        assert!(grammar_for_response_format(None).is_none());
        assert!(
            grammar_for_response_format(Some(&object(&[("type", string("json_object"))])))
                .is_some()
        );
        assert!(grammar_for_response_format(Some(&object(&[("nope", string("x"))]))).is_none());
    }

    #[test]
    fn the_system_block_lists_the_tools_and_call_format() {
        let block = tool_system_block(&[ToolSpec::new(
            "add",
            "adds numbers",
            object(&[("type", string("object"))]),
        )]);
        assert!(block.contains("- add: adds numbers Parameters schema:"));
        assert!(block.contains("<tool_call>"));
        assert!(block.contains("</tool_call>"));
    }

    #[test]
    fn a_name_with_a_quote_is_escaped_in_the_grammar() {
        // A property name containing a backslash is escaped, not rejected (only
        // quotes/newlines/backslashes in the *set* {"\\n} are rejected — a
        // backslash is rejected too, so use a plain name and check escaping via
        // an enum value instead).
        let params = object(&[
            ("type", string("object")),
            (
                "properties",
                object(&[(
                    "q",
                    object(&[("enum", JsonValue::Array(vec![string("a\"b")]))]),
                )]),
            ),
            ("required", JsonValue::Array(vec![string("q")])),
        ]);
        let grammar = tool_grammar(&[spec("t", params)]).unwrap();
        // The enum value's inner quote is escaped for the GBNF literal (matching
        // the Swift builder byte-for-byte).
        assert!(grammar.contains(r#""\"a\"b\"""#), "{grammar}");
    }
}
