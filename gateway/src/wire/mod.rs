//! The wire layer: decoding OpenAI/Ollama request bodies into the kernel's chat
//! model and encoding the kernel's output back into each dialect's response shape.

pub mod base64;
pub mod openai;
pub mod param_decoding;
