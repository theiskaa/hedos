//! The wire layer: decoding OpenAI/Ollama/Anthropic request bodies into the
//! kernel's chat model and encoding the kernel's output back into each dialect's
//! response shape.

pub mod anthropic;
pub mod multipart;
pub mod ollama;
pub mod openai;
pub mod param_decoding;
pub mod timestamp;
