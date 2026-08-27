//! The HTTP shapes the CLI probes with: a GET decoded as JSON, either `None`
//! on any failure, or with silence told apart from absence.

use std::time::Duration;

use serde::de::DeserializeOwned;

/// GET `url` and decode the JSON body: `Ok(None)` when nothing listens
/// there, `Err` when something does but fails to answer in `timeout` or
/// answers badly. For the decisions where "not running" and "too busy to
/// say" must not be confused.
pub(crate) async fn probe_json_answered<T: DeserializeOwned>(
    url: &str,
    timeout: Duration,
) -> Result<Option<T>, String> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|error| error.to_string())?;
    let response = match client.get(url).send().await {
        Ok(response) => response,
        Err(error) if error.is_connect() => return Ok(None),
        Err(error) if error.is_timeout() => {
            return Err(format!(
                "no answer from {url} within {}s",
                timeout.as_secs()
            ));
        }
        Err(error) => return Err(error.to_string()),
    };
    response
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map(Some)
        .map_err(|error| error.to_string())
}

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
