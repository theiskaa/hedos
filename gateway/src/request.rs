//! A parsed HTTP request as the gateway sees it: method, normalized path, query,
//! headers, and body, plus the small accessors the handlers need (bearer token,
//! JSON body). The HTTP server constructs one of these from its own types.

use std::collections::BTreeMap;

use kernel::records::JsonValue;

use crate::error::{GatewayError, GatewayErrorKind};

/// A parsed request.
#[derive(Debug, Clone)]
pub struct GatewayRequest {
    /// The upper-cased HTTP method.
    pub method: String,
    /// The normalized path (no trailing slash except for the root `/`).
    pub path: String,
    /// The decoded query parameters.
    pub query: BTreeMap<String, String>,
    /// The headers, in arrival order, with their original names.
    pub headers: Vec<(String, String)>,
    /// The raw request body.
    pub body: Vec<u8>,
}

impl GatewayRequest {
    /// Parse a request from its method, request-target URI, headers, and body.
    pub fn new(method: &str, uri: &str, headers: Vec<(String, String)>, body: Vec<u8>) -> Self {
        let (raw_path, raw_query) = match uri.split_once('?') {
            Some((path, query)) => (path, query),
            None => (uri, ""),
        };

        let mut path = percent_decode(raw_path);
        if path.len() > 1 && path.ends_with('/') {
            path.pop();
        }
        if path.is_empty() {
            path = "/".to_owned();
        }

        let mut query = BTreeMap::new();
        for pair in raw_query.split('&').filter(|pair| !pair.is_empty()) {
            let (name, value) = match pair.split_once('=') {
                Some((name, value)) => (name, value),
                None => (pair, ""),
            };
            query.insert(percent_decode(name), percent_decode(value));
        }

        Self {
            method: method.to_uppercase(),
            path,
            query,
            headers,
            body,
        }
    }

    /// The value of the first header named `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The API token: a `Bearer` authorization value, else the `x-api-key`
    /// header, trimmed of surrounding whitespace.
    pub fn bearer_token(&self) -> Option<String> {
        if let Some(authorization) = self.header("Authorization") {
            let mut parts = authorization.splitn(2, ' ');
            // An empty remainder (`"Bearer "`) is not a token: Swift's
            // whitespace split omits it, so we fall through to `x-api-key`.
            if let (Some(scheme), Some(token)) = (parts.next(), parts.next())
                && scheme.eq_ignore_ascii_case("Bearer")
                && !token.is_empty()
            {
                return Some(token.trim().to_owned());
            }
        }
        self.header("x-api-key").map(|key| key.trim().to_owned())
    }

    /// The request body decoded as a JSON object, or a bad-request error if it is
    /// empty or not a JSON object.
    pub fn decoded_json(&self) -> Result<BTreeMap<String, JsonValue>, GatewayError> {
        let invalid = || {
            GatewayError::new(
                GatewayErrorKind::BadRequest,
                "request body must be a JSON object",
            )
        };
        if self.body.is_empty() {
            return Err(invalid());
        }
        match serde_json::from_slice::<JsonValue>(&self.body) {
            Ok(JsonValue::Object(fields)) => Ok(fields),
            _ => Err(invalid()),
        }
    }
}

/// Percent-decode a URI component (`%XX` escapes only; invalid escapes are left
/// as-is). Bytes are reassembled and interpreted as UTF-8, lossily.
fn percent_decode(input: &str) -> String {
    if !input.contains('%') {
        return input.to_owned();
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            out.push(high << 4 | low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: &str, uri: &str) -> GatewayRequest {
        GatewayRequest::new(method, uri, Vec::new(), Vec::new())
    }

    #[test]
    fn the_method_is_upper_cased() {
        assert_eq!(request("post", "/x").method, "POST");
    }

    #[test]
    fn a_trailing_slash_is_trimmed_except_at_the_root() {
        assert_eq!(request("GET", "/v1/models/").path, "/v1/models");
        assert_eq!(request("GET", "/").path, "/");
        assert_eq!(request("GET", "").path, "/");
    }

    #[test]
    fn the_query_is_split_and_decoded() {
        let request = request("GET", "/api/tags?name=llama%203&stream=true");
        assert_eq!(request.path, "/api/tags");
        assert_eq!(
            request.query.get("name").map(String::as_str),
            Some("llama 3")
        );
        assert_eq!(
            request.query.get("stream").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn a_query_key_with_no_value_is_empty() {
        let request = request("GET", "/x?flag");
        assert_eq!(request.query.get("flag").map(String::as_str), Some(""));
    }

    #[test]
    fn headers_are_matched_case_insensitively() {
        let request = GatewayRequest::new(
            "GET",
            "/x",
            vec![("Content-Type".to_owned(), "application/json".to_owned())],
            Vec::new(),
        );
        assert_eq!(request.header("content-type"), Some("application/json"));
        assert_eq!(request.header("missing"), None);
    }

    #[test]
    fn a_bearer_token_is_extracted_and_trimmed() {
        let request = GatewayRequest::new(
            "GET",
            "/x",
            vec![("Authorization".to_owned(), "Bearer   sk-abc  ".to_owned())],
            Vec::new(),
        );
        assert_eq!(request.bearer_token().as_deref(), Some("sk-abc"));
    }

    #[test]
    fn the_api_key_header_is_a_fallback() {
        let request = GatewayRequest::new(
            "GET",
            "/x",
            vec![("x-api-key".to_owned(), " key123 ".to_owned())],
            Vec::new(),
        );
        assert_eq!(request.bearer_token().as_deref(), Some("key123"));
    }

    #[test]
    fn a_bearer_scheme_with_an_empty_token_falls_through_to_the_api_key() {
        let request = GatewayRequest::new(
            "GET",
            "/x",
            vec![
                ("Authorization".to_owned(), "Bearer ".to_owned()),
                ("x-api-key".to_owned(), "realkey".to_owned()),
            ],
            Vec::new(),
        );
        assert_eq!(request.bearer_token().as_deref(), Some("realkey"));
    }

    #[test]
    fn a_non_bearer_authorization_is_ignored() {
        let request = GatewayRequest::new(
            "GET",
            "/x",
            vec![("Authorization".to_owned(), "Basic abc".to_owned())],
            Vec::new(),
        );
        assert_eq!(request.bearer_token(), None);
    }

    #[test]
    fn a_json_body_decodes_to_an_object() {
        let request = GatewayRequest::new("POST", "/x", Vec::new(), br#"{"model":"m"}"#.to_vec());
        let fields = request.decoded_json().unwrap();
        assert_eq!(
            fields.get("model"),
            Some(&JsonValue::String("m".to_owned()))
        );
    }

    #[test]
    fn an_empty_or_non_object_body_is_rejected() {
        assert!(request("POST", "/x").decoded_json().is_err());
        let array = GatewayRequest::new("POST", "/x", Vec::new(), b"[1,2]".to_vec());
        assert!(array.decoded_json().is_err());
    }
}
