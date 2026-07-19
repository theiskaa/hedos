//! A minimal `multipart/form-data` reader for the file-upload endpoints. It
//! parses the boundary out of a `Content-Type` and splits a body into its named
//! parts; it does no streaming and holds the whole body in memory.

/// One parsed part: its form field `name`, optional `filename`, and raw bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// The `name` from the part's `Content-Disposition`, if any.
    pub name: Option<String>,
    /// The `filename` from the part's `Content-Disposition`, if any.
    pub filename: Option<String>,
    /// The part's raw body bytes.
    pub data: Vec<u8>,
}

/// The `boundary=` value from a `multipart/form-data` content type, unquoted.
/// Returns `None` when the type is not multipart or carries no boundary.
pub fn boundary(content_type: Option<&str>) -> Option<String> {
    let content_type = content_type?;
    if !content_type.to_lowercase().contains("multipart/form-data") {
        return None;
    }
    for component in content_type.split(';') {
        let trimmed = component.trim();
        if !trimmed.to_lowercase().starts_with("boundary=") {
            continue;
        }
        let value = unquote(&trimmed["boundary=".len()..]);
        return (!value.is_empty()).then(|| value.to_owned());
    }
    None
}

/// Split `body` into its parts at the given `boundary`.
pub fn parse(body: &[u8], boundary: &str) -> Vec<Part> {
    let delimiter = format!("--{boundary}").into_bytes();
    let mut segments: Vec<&[u8]> = Vec::new();
    let mut search_start = 0;
    let mut previous_end: Option<usize> = None;
    while let Some(offset) = find(&body[search_start..], &delimiter) {
        let lower = search_start + offset;
        let upper = lower + delimiter.len();
        if let Some(previous) = previous_end {
            segments.push(&body[previous..lower]);
        }
        previous_end = Some(upper);
        search_start = upper;
    }
    segments.into_iter().filter_map(part).collect()
}

/// Parse one boundary-delimited segment into a [`Part`]. The closing `--`
/// segment and anything without a header block yield `None`.
fn part(segment: &[u8]) -> Option<Part> {
    let mut slice = segment;
    if slice.starts_with(b"--") {
        return None;
    }
    if slice.starts_with(b"\r\n") {
        slice = &slice[2..];
    }
    let header_end = find(slice, b"\r\n\r\n")?;
    let header_data = &slice[..header_end];
    let mut content = &slice[header_end + 4..];
    if content.ends_with(b"\r\n") {
        content = &content[..content.len() - 2];
    }
    let (name, filename) = disposition(&String::from_utf8_lossy(header_data));
    Some(Part {
        name,
        filename,
        data: content.to_vec(),
    })
}

/// Read the `name`/`filename` fields out of a part's header block.
fn disposition(headers: &str) -> (Option<String>, Option<String>) {
    let mut name = None;
    let mut filename = None;
    for line in headers.split("\r\n") {
        if !line.to_lowercase().starts_with("content-disposition:") {
            continue;
        }
        for token in line.split(';') {
            let trimmed = token.trim();
            if let Some(value) = field_value(trimmed, "name") {
                name = Some(value);
            }
            if let Some(value) = field_value(trimmed, "filename") {
                filename = Some(value);
            }
        }
    }
    (name, filename)
}

/// The unquoted value of `key=…` in a `Content-Disposition` token.
fn field_value(token: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    token
        .strip_prefix(&prefix)
        .map(|value| unquote(value).to_owned())
}

/// Strip one pair of surrounding double quotes, if present.
fn unquote(value: &str) -> &str {
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

/// The first offset of `needle` within `haystack`, if any.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quoted_boundary_is_unquoted() {
        assert_eq!(
            boundary(Some("multipart/form-data; boundary=\"abc123\"")).as_deref(),
            Some("abc123")
        );
        assert_eq!(
            boundary(Some("multipart/form-data; boundary=xyz")).as_deref(),
            Some("xyz")
        );
    }

    #[test]
    fn a_non_multipart_type_has_no_boundary() {
        assert_eq!(boundary(Some("application/json")), None);
        assert_eq!(boundary(None), None);
        assert_eq!(boundary(Some("multipart/form-data")), None);
    }

    #[test]
    fn parse_splits_named_parts_and_a_file() {
        let boundary = "BOUND";
        let body = concat!(
            "--BOUND\r\n",
            "Content-Disposition: form-data; name=\"model\"\r\n\r\n",
            "whisper\r\n",
            "--BOUND\r\n",
            "Content-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n",
            "Content-Type: audio/wav\r\n\r\n",
            "RIFFDATA\r\n",
            "--BOUND--\r\n",
        );
        let parts = parse(body.as_bytes(), boundary);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].name.as_deref(), Some("model"));
        assert_eq!(parts[0].data, b"whisper");
        assert_eq!(parts[1].name.as_deref(), Some("file"));
        assert_eq!(parts[1].filename.as_deref(), Some("a.wav"));
        assert_eq!(parts[1].data, b"RIFFDATA");
    }

    #[test]
    fn binary_content_survives_intact() {
        let boundary = "B";
        let mut body = Vec::new();
        body.extend_from_slice(b"--B\r\nContent-Disposition: form-data; name=\"file\"\r\n\r\n");
        let payload = [0u8, 13, 10, 255, 0, 1];
        body.extend_from_slice(&payload);
        body.extend_from_slice(b"\r\n--B--\r\n");
        let parts = parse(&body, boundary);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].data, payload);
    }
}
