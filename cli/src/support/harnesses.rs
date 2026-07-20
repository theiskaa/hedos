//! The coding harnesses `hedos launch` knows how to wire, and the environment,
//! arguments, and generated config each one needs to reach the local gateway.
//!
//! Every harness is described by data plus one pure `plan` function, so the wiring
//! is unit-testable without spawning anything. `commands::launch` is the shell
//! that writes the files and runs the process.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::json;

/// The wire dialect a harness speaks to the gateway.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dialect {
    /// OpenAI chat completions, under `/v1`.
    OpenAi,
    /// Anthropic Messages, under `/v1/messages`.
    Anthropic,
}

impl Dialect {
    /// The dialect's wire name, as reported by `--json`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
        }
    }
}

/// Which wiring recipe a harness uses. `plan` matches this exhaustively, so a
/// harness added to [`HARNESSES`] without wiring is a compile error rather than
/// a silently empty plan.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HarnessKind {
    Claude,
    OpenCode,
    Aider,
    Goose,
    Crush,
}

/// One launchable harness.
pub struct HarnessSpec {
    /// The wiring recipe used by `plan`.
    kind: HarnessKind,
    /// The name used on the command line.
    pub key: &'static str,
    /// The executable to look for on `PATH`.
    pub binary: &'static str,
    /// The name shown in the picker.
    pub display: &'static str,
    /// Where to read more when the binary is missing.
    pub homepage: &'static str,
    /// The dialect the gateway must serve for this harness to work.
    pub dialect: Dialect,
    /// Whether the harness drives the model through tool calls. All of them do
    /// except aider, whose edit formats are plain text in the assistant message,
    /// so it is the one that still works on a model without tool support.
    pub needs_tools: bool,
}

/// Everything needed to spawn one harness.
pub struct LaunchPlan {
    /// Extra arguments appended after the user's own.
    pub args: Vec<String>,
    /// Environment layered over the inherited environment.
    pub env: BTreeMap<String, String>,
    /// Config files to write before spawning, as (path, contents).
    pub files: Vec<(PathBuf, String)>,
}

/// The harnesses this build knows about, in picker order.
pub const HARNESSES: &[HarnessSpec] = &[
    HarnessSpec {
        kind: HarnessKind::Claude,
        key: "claude",
        binary: "claude",
        display: "Claude Code",
        homepage: "https://claude.com/claude-code",
        dialect: Dialect::Anthropic,
        needs_tools: true,
    },
    HarnessSpec {
        kind: HarnessKind::OpenCode,
        key: "opencode",
        binary: "opencode",
        display: "OpenCode",
        homepage: "https://opencode.ai",
        dialect: Dialect::OpenAi,
        needs_tools: true,
    },
    HarnessSpec {
        kind: HarnessKind::Aider,
        key: "aider",
        binary: "aider",
        display: "Aider",
        homepage: "https://aider.chat",
        dialect: Dialect::OpenAi,
        needs_tools: false,
    },
    HarnessSpec {
        kind: HarnessKind::Goose,
        key: "goose",
        binary: "goose",
        display: "Goose",
        homepage: "https://block.github.io/goose",
        dialect: Dialect::OpenAi,
        needs_tools: true,
    },
    HarnessSpec {
        kind: HarnessKind::Crush,
        key: "crush",
        binary: "crush",
        display: "Crush",
        homepage: "https://github.com/charmbracelet/crush",
        dialect: Dialect::OpenAi,
        needs_tools: true,
    },
];

/// The harness registered under `key`, if any.
pub fn find(key: &str) -> Option<&'static HarnessSpec> {
    HARNESSES
        .iter()
        .find(|harness| harness.key.eq_ignore_ascii_case(key))
}

/// The gateway credential handed to every harness. The gateway treats all local
/// callers as trusted, but most harnesses refuse to start without *some* key set,
/// so this is a placeholder rather than a secret.
const TOKEN: &str = "hedos";

impl HarnessSpec {
    /// The wiring for this harness against a gateway on `port`, opening on
    /// `model` with `available` offered in its model picker.
    ///
    /// The harnesses configured through a file have to be told every model by
    /// name — neither opencode nor crush discovers them — so listing only the
    /// selected one would leave the picker with a single entry and no way to
    /// switch. The env-configured harnesses take a model name at a time and
    /// ignore `available`.
    ///
    /// `config_dir` is where generated config files are written; only the harnesses
    /// that cannot be configured through the environment use it.
    pub fn plan(
        &self,
        port: u16,
        model: &str,
        available: &[String],
        config_dir: &std::path::Path,
    ) -> LaunchPlan {
        let origin = format!("http://127.0.0.1:{port}");
        let v1 = format!("{origin}/v1");
        let mut env = BTreeMap::new();
        let mut args = Vec::new();
        let mut files = Vec::new();

        match self.kind {
            HarnessKind::Claude => {
                // Claude Code appends `/v1/messages` itself, so the base URL
                // carries no version segment.
                env.insert("ANTHROPIC_BASE_URL".to_owned(), origin);
                env.insert("ANTHROPIC_AUTH_TOKEN".to_owned(), TOKEN.to_owned());
                env.insert("ANTHROPIC_MODEL".to_owned(), model.to_owned());
                // Pre-release capabilities pair a beta header with a body field
                // the gateway does not implement; sending neither is a clean
                // degrade, sending one half is a hard 400.
                env.insert(
                    "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS".to_owned(),
                    "1".to_owned(),
                );
            }
            HarnessKind::OpenCode => {
                // Inline config rather than a file, so the user's own
                // opencode.json is neither read around nor written to.
                env.insert(
                    "OPENCODE_CONFIG_CONTENT".to_owned(),
                    opencode_config(&v1, model, available).to_string(),
                );
                env.insert("OPENCODE_MODEL".to_owned(), format!("hedos/{model}"));
            }
            HarnessKind::Aider => {
                env.insert("OPENAI_API_BASE".to_owned(), v1);
                env.insert("OPENAI_API_KEY".to_owned(), TOKEN.to_owned());
                // The `openai/` prefix is what routes aider's LiteLLM layer at
                // the chat-completions endpoint rather than a named provider.
                args.push("--model".to_owned());
                args.push(format!("openai/{model}"));
            }
            HarnessKind::Goose => {
                env.insert("GOOSE_PROVIDER".to_owned(), "openai".to_owned());
                env.insert("GOOSE_MODEL".to_owned(), model.to_owned());
                env.insert("OPENAI_HOST".to_owned(), origin);
                // Goose picks the Responses API unless the base path names
                // chat/completions outright.
                env.insert(
                    "OPENAI_BASE_PATH".to_owned(),
                    "v1/chat/completions".to_owned(),
                );
                env.insert("OPENAI_API_KEY".to_owned(), TOKEN.to_owned());
            }
            HarnessKind::Crush => {
                let path = config_dir.join("crush.json");
                files.push((
                    path.clone(),
                    crush_config(&v1, model, available).to_string(),
                ));
                env.insert(
                    "CRUSH_GLOBAL_CONFIG".to_owned(),
                    path.to_string_lossy().into_owned(),
                );
            }
        }

        LaunchPlan { args, env, files }
    }
}

/// An opencode provider block pointing at the gateway.
///
/// Every model has to be declared: opencode does no discovery, so one left out
/// of this map simply does not exist as far as its picker is concerned.
fn opencode_config(base_url: &str, model: &str, available: &[String]) -> serde_json::Value {
    let models: serde_json::Map<String, serde_json::Value> = available
        .iter()
        .map(|name| (name.clone(), json!({ "name": name })))
        .collect();
    json!({
        "$schema": "https://opencode.ai/config.json",
        "provider": {
            "hedos": {
                "npm": "@ai-sdk/openai-compatible",
                "name": "Hedos",
                "options": { "baseURL": base_url, "apiKey": TOKEN },
                "models": models,
            }
        },
        "model": format!("hedos/{model}"),
    })
}

/// A crush provider block pointing at the gateway, opening on `model`.
///
/// The type is `openai-compat` rather than `openai` deliberately: the `openai`
/// type routes to the Responses API for any model id containing `gpt-4` or
/// `gpt-5`, and no config flag turns that off. The `models` selection block is
/// what makes `-m` real for crush — without it, crush opens on its own
/// persisted choice and the named model is ignored. Both roles point at the
/// same local model; there is no separate small one to hand the light tasks.
fn crush_config(base_url: &str, model: &str, available: &[String]) -> serde_json::Value {
    let models: Vec<serde_json::Value> = available
        .iter()
        .map(|name| json!({ "id": name, "name": name }))
        .collect();
    let selected = json!({ "model": model, "provider": "hedos" });
    json!({
        "$schema": "https://charm.land/crush.json",
        "providers": {
            "hedos": {
                "type": "openai-compat",
                "base_url": base_url,
                "api_key": TOKEN,
                "models": models,
            }
        },
        "models": {
            "large": selected.clone(),
            "small": selected,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn plan_for(key: &str) -> LaunchPlan {
        let spec = find(key).expect("harness is registered");
        let available = ["qwen3".to_owned(), "gemma3".to_owned()];
        spec.plan(4321, "qwen3", &available, Path::new("/tmp/hedos-launch"))
    }

    #[test]
    fn harness_lookup_ignores_case_and_rejects_unknowns() {
        assert_eq!(find("OpenCode").map(|a| a.key), Some("opencode"));
        assert!(find("cursor").is_none());
    }

    #[test]
    fn claude_gets_a_versionless_base_url() {
        let plan = plan_for("claude");
        // Claude Code appends /v1/messages itself; a /v1 here would double it.
        assert_eq!(
            plan.env.get("ANTHROPIC_BASE_URL").map(String::as_str),
            Some("http://127.0.0.1:4321")
        );
        assert_eq!(
            plan.env.get("ANTHROPIC_MODEL").map(String::as_str),
            Some("qwen3")
        );
    }

    #[test]
    fn every_harness_but_aider_drives_the_model_through_tools() {
        // aider's edit formats are plain text in the assistant message, so it is
        // the fallback when a model has no tool support; the rest cannot work
        // without it, and the launch pre-flight probes for it accordingly.
        let toolless: Vec<&str> = HARNESSES
            .iter()
            .filter(|harness| !harness.needs_tools)
            .map(|harness| harness.key)
            .collect();
        assert_eq!(toolless, vec!["aider"]);
    }

    #[test]
    fn openai_harnesses_get_a_versioned_base_url() {
        let plan = plan_for("aider");
        assert_eq!(
            plan.env.get("OPENAI_API_BASE").map(String::as_str),
            Some("http://127.0.0.1:4321/v1")
        );
        assert_eq!(plan.args, vec!["--model", "openai/qwen3"]);
    }

    #[test]
    fn goose_is_pinned_to_the_chat_completions_path() {
        let plan = plan_for("goose");
        // Any other base path lets goose fall through to the Responses API.
        assert_eq!(
            plan.env.get("OPENAI_BASE_PATH").map(String::as_str),
            Some("v1/chat/completions")
        );
        assert_eq!(
            plan.env.get("OPENAI_HOST").map(String::as_str),
            Some("http://127.0.0.1:4321")
        );
    }

    #[test]
    fn crush_is_configured_as_openai_compat() {
        let plan = plan_for("crush");
        let (path, body) = plan.files.first().expect("crush needs a config file");
        assert!(path.ends_with("crush.json"));
        let value: serde_json::Value = serde_json::from_str(body).expect("valid json");
        // `openai` would route gpt-4/gpt-5-shaped ids at the Responses API.
        assert_eq!(value["providers"]["hedos"]["type"], "openai-compat");
        assert_eq!(
            value["providers"]["hedos"]["base_url"],
            "http://127.0.0.1:4321/v1"
        );
        assert_eq!(
            plan.env.get("CRUSH_GLOBAL_CONFIG").map(String::as_str),
            Some(path.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn opencode_declares_every_model_and_opens_on_the_chosen_one() {
        let plan = plan_for("opencode");
        let content = plan
            .env
            .get("OPENCODE_CONFIG_CONTENT")
            .expect("opencode is configured inline");
        let value: serde_json::Value = serde_json::from_str(content).expect("valid json");
        assert_eq!(value["model"], "hedos/qwen3");
        // opencode does no discovery, so a model left out of this map cannot be
        // switched to from its picker at all.
        let models = value["provider"]["hedos"]["models"]
            .as_object()
            .expect("a models map");
        assert_eq!(models.len(), 2);
        assert!(models.contains_key("qwen3"));
        assert!(models.contains_key("gemma3"));
        assert!(plan.files.is_empty());
    }

    #[test]
    fn crush_opens_on_the_chosen_model() {
        let plan = plan_for("crush");
        let (_, body) = plan.files.first().expect("crush needs a config file");
        let value: serde_json::Value = serde_json::from_str(body).expect("valid json");
        // Without this selection block, `-m` is ignored and crush opens on its
        // own persisted choice.
        assert_eq!(value["models"]["large"]["model"], "qwen3");
        assert_eq!(value["models"]["large"]["provider"], "hedos");
        assert_eq!(value["models"]["small"]["model"], "qwen3");
    }

    #[test]
    fn crush_declares_every_model_too() {
        let plan = plan_for("crush");
        let (_, body) = plan.files.first().expect("crush needs a config file");
        let value: serde_json::Value = serde_json::from_str(body).expect("valid json");
        let models = value["providers"]["hedos"]["models"]
            .as_array()
            .expect("a models array");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["id"], "qwen3");
        assert_eq!(models[1]["id"], "gemma3");
    }

    #[test]
    fn only_claude_asks_for_the_anthropic_dialect() {
        let anthropic: Vec<&str> = HARNESSES
            .iter()
            .filter(|a| a.dialect == Dialect::Anthropic)
            .map(|a| a.key)
            .collect();
        assert_eq!(anthropic, vec!["claude"]);
    }
}
