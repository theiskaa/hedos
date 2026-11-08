//! Telling the turns of one conversation apart from another's.
//!
//! Clients resend the whole history every turn and carry no conversation id,
//! so the opening of the conversation stands in for one: the system prompt and
//! the first user message do not change as the conversation grows.

use std::hash::{DefaultHasher, Hash, Hasher};

use crate::capabilities::chat::{ChatMessage, ChatRole};

/// A key equal across every turn of one conversation, or `None` when there is
/// no user message to anchor it.
///
/// The key lives only in memory for the life of the gateway, so the standard
/// hasher (fixed keys within one build) is stable enough.
pub fn conversation_key(messages: &[ChatMessage]) -> Option<u64> {
    let first_user = messages
        .iter()
        .position(|message| message.role == ChatRole::User)?;
    let mut hasher = DefaultHasher::new();
    for message in &messages[..first_user] {
        if message.role == ChatRole::System {
            message.content.hash(&mut hasher);
        }
    }
    messages[first_user].content.hash(&mut hasher);
    Some(hasher.finish())
}
