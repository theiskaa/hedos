//! The vocabulary types that identify a model and its runtime.
//!
//! `Modality`, `Capability`, `SourceKind`, and `RuntimeId` are open string sets
//! (new values can appear without a code change), modeled as string newtypes with
//! constructors for the well-known values. The remaining enums are closed.

macro_rules! string_id {
    (
        $(#[$meta:meta])*
        $name:ident { $( $ctor:ident => $value:literal ),* $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord,
            serde::Serialize, serde::Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            $(
                #[doc = concat!("The well-known `", $value, "` value.")]
                pub fn $ctor() -> Self {
                    Self(String::from($value))
                }
            )*

            /// The underlying string value.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

string_id! {
    /// A model's primary output modality.
    Modality {
        unknown => "unknown",
        text => "text",
        image => "image",
        speech => "speech",
        audio => "audio",
        video => "video",
        vision => "vision",
        embedding => "embedding",
    }
}

string_id! {
    /// Something a model can be asked to do.
    Capability {
        chat => "chat",
        complete => "complete",
        embed => "embed",
        see => "see",
        image => "image",
        speak => "speak",
        transcribe => "transcribe",
        tools => "tools",
    }
}

string_id! {
    /// Where a model was found or installed from.
    SourceKind {
        ollama => "ollama",
        huggingface_cache => "huggingface-cache",
        lm_studio => "lm-studio",
        builtin => "builtin",
        endpoint => "endpoint",
        file => "file",
        folder => "folder",
    }
}

string_id! {
    /// The identifier of a runtime adapter that can execute a model.
    RuntimeId {
        llama_cpp => "llama-cpp",
        whisper_cpp => "whisper-cpp",
        ollama => "ollama",
        mlx_swift => "mlx-swift",
        apple_foundation => "apple-foundation",
        openai_endpoint => "generic:openai-server",
        mflux => "python:mflux",
        diffusers => "python:diffusers",
        mlx_lm => "python:mlx-lm",
        mlx_audio => "python:mlx-audio",
        mlx_vlm => "python:mlx-vlm",
        embeddings => "python:embeddings",
        comfy_ui => "comfyui",
        a1111 => "a1111",
    }
}

/// How a runtime delivers a model's output.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionMode {
    /// Tokens stream back incrementally.
    Stream,
    /// A long-running job produces an artifact.
    Job,
    /// A single synchronous request/response.
    #[default]
    Sync,
}

/// How much support a model needs before it can run.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum RunTier {
    /// Runs directly, no extra runtime to install.
    Native,
    /// Runs via a managed sidecar the app provisions.
    Managed,
    /// Runs on a remote endpoint.
    Remote,
    /// Needs a runtime recipe that is not yet available.
    #[default]
    RecipeNeeded,
}

/// The lifecycle state of a model record.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum ModelState {
    /// Resolved to a runtime and present on disk.
    Ready,
    /// Not yet resolved to a runtime.
    #[default]
    Unresolved,
    /// Known but its weights are no longer on disk.
    Missing,
}

/// The bid preference numbers runtime adapters use to compete for a model. Lower
/// wins. This is the single global ordering; adapters must not mint their own.
pub struct BidPreference;

impl BidPreference {
    /// llama.cpp GGUF text runtime.
    pub const LLAMA_CPP: i64 = 10;
    /// whisper.cpp transcription runtime.
    pub const WHISPER_CPP: i64 = 10;
    /// OpenAI-compatible remote endpoint.
    pub const ENDPOINT: i64 = 10;
    /// mlx-vlm vision-language sidecar.
    pub const MLX_VLM: i64 = 14;
    /// in-process MLX-Swift text runtime.
    pub const MLX_SWIFT: i64 = 15;
    /// Apple Foundation Models.
    pub const APPLE_FOUNDATION: i64 = 15;
    /// Ollama daemon.
    pub const OLLAMA: i64 = 20;
    /// mflux FLUX image runtime.
    pub const MFLUX: i64 = 25;
    /// diffusers image runtime.
    pub const DIFFUSERS: i64 = 26;
    /// ComfyUI daemon.
    pub const COMFY_UI: i64 = 27;
    /// Automatic1111 daemon.
    pub const A1111: i64 = 28;
    /// mlx-audio speech runtime.
    pub const MLX_AUDIO: i64 = 30;
    /// embeddings sidecar.
    pub const EMBEDDINGS: i64 = 32;
    /// mlx-lm text sidecar.
    pub const MLX_LM: i64 = 40;
    /// Manifest-declared runtime (lowest priority).
    pub const MANIFEST: i64 = 100;
}
