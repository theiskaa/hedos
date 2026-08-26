//! The one HTTP shape the CLI probes with: a GET decoded as JSON, or `None`
//! when nothing answers in time.

use std::time::Duration;

use serde::de::DeserializeOwned;

/// GET `url` and decode the JSON body, or `None` on any failure or after
/// `timeout`.
pub(crate) async fn probe_json<T: DeserializeOwned>(url: &str, timeout: Duration) -> Option<T> {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .ok()?
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()
}
