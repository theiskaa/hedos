//! How a pull's record reads, wherever it is shown. The command line puts these
//! in a table and the screen puts them in the task strip, and a pull should not
//! describe itself differently depending on which one is looking.

use kernel::install::pulls::{PullEvent, PullEventKind, PullJobDir, PullStatus, START_GRACE_MS};
use kernel::records::byte_format::format_bytes;

use crate::support::clock;
use crate::support::table::DASH;

/// How wide a note is allowed to be before it is cut short; a provider's message
/// can run to a paragraph and the surfaces showing it are one line each.
const NOTE_LIMIT: usize = 44;

/// How far along a pull is: a percentage against a firm total, the bytes alone
/// when the total is only an estimate, and a dash before anything has moved.
pub fn progress(status: &PullStatus) -> String {
    let done = format_bytes(status.progress.bytes_downloaded);
    match (status.progress.fraction(), status.progress.total_bytes) {
        (Some(fraction), Some(total)) => format!(
            "{}%  {done} of {}",
            (fraction * 100.0) as u64,
            format_bytes(total)
        ),
        _ if status.progress.bytes_downloaded > 0 => done,
        _ => DASH.to_owned(),
    }
}

/// The one thing worth saying about a pull beside its state: that no worker took
/// it up, else when the next attempt is due, else why it stopped, else what a
/// still-running provider last said, else which attempt is running.
pub fn note(job: &PullJobDir, status: &PullStatus, now_ms: i64) -> String {
    // A queued job waiting behind `max_concurrent` and one nothing ever came for
    // read the same in the record, and only one of them is going to move.
    if job.abandoned(now_ms, START_GRACE_MS) {
        return "no worker".to_owned();
    }
    if let Some(due) = status.next_attempt_at_ms {
        return format!("retry in {}", clock::millis(due.saturating_sub(now_ms)));
    }
    if let Some(message) = &status.message {
        return clip(message);
    }
    // A provider's last line is what it was doing, which is worth reading only
    // while it is still doing it.
    if let Some(line) = &status.status_line
        && status.state.is_live()
    {
        return clip(line);
    }
    if status.attempt > 1 {
        return format!("attempt {}", status.attempt);
    }
    String::new()
}

/// One line of history: how long ago it happened, then what happened.
pub fn event_line(event: &PullEvent, now_ms: i64) -> String {
    let ago = clock::millis(now_ms.saturating_sub(event.at_ms));
    let what = match &event.kind {
        PullEventKind::State { state } => state.to_string(),
        PullEventKind::Status { text } => text.clone(),
        PullEventKind::Retry {
            attempt,
            reason,
            delay_ms,
        } => format!(
            "attempt {attempt} failed, retrying in {}: {reason}",
            clock::millis(*delay_ms)
        ),
    };
    format!("{ago:>5} ago  {what}")
}

/// `text` at [`NOTE_LIMIT`] characters, with an ellipsis when it was cut.
fn clip(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= NOTE_LIMIT {
        return text.to_owned();
    }
    let kept: String = text.chars().take(NOTE_LIMIT - 1).collect();
    format!("{}…", kept.trim_end())
}

#[cfg(test)]
pub mod testing;

#[cfg(test)]
mod tests;
