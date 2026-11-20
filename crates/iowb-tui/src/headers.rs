use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};

/// A header sent with every request to the server, for example the service token
/// an access proxy such as Cloudflare Access asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

impl Header {
    pub fn blank() -> Self {
        Self::default()
    }

    fn is_blank(&self) -> bool {
        self.name.trim().is_empty() && self.value.trim().is_empty()
    }
}

/// The two headers a Cloudflare Access service token is sent in.
pub const CLOUDFLARE_ACCESS: [&str; 2] = ["CF-Access-Client-Id", "CF-Access-Client-Secret"];

/// Checks editor rows the way the mobile app does: blank rows are ignored, every
/// other row needs a valid name and a value, and a repeated name (in any case)
/// keeps its last value. Row numbers in errors count blank rows too.
pub fn validate(rows: &[Header]) -> Result<Vec<Header>, String> {
    let mut headers: Vec<Header> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        if row.is_blank() {
            continue;
        }
        let number = index + 1;
        let (name, value) = (row.name.trim(), row.value.trim());
        if name.is_empty() {
            return Err(format!("Header {number} needs a name."));
        }
        if HeaderName::from_bytes(name.as_bytes()).is_err() {
            return Err(format!("Header {number} has an invalid name."));
        }
        if value.is_empty() {
            return Err(format!("Header {number} needs a value."));
        }
        if value.contains(['\r', '\n']) || HeaderValue::from_str(value).is_err() {
            return Err(format!("Header {number} has an invalid value."));
        }
        headers.retain(|other| !other.name.eq_ignore_ascii_case(name));
        headers.push(Header {
            name: name.to_string(),
            value: value.to_string(),
        });
    }
    Ok(headers)
}

/// The headers as an HTTP header map. Entries that are not valid (a hand-edited
/// settings file, say) are left out instead of failing every request.
pub fn header_map(headers: &[Header]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for header in headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(header.name.as_bytes()),
            HeaderValue::from_str(&header.value),
        ) {
            map.insert(name, value);
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, value: &str) -> Header {
        Header {
            name: name.into(),
            value: value.into(),
        }
    }

    #[test]
    fn blank_rows_are_ignored_and_the_rest_is_trimmed() {
        let rows = [row("", ""), row(" X-Team ", " blue "), row("  ", "")];
        assert_eq!(validate(&rows), Ok(vec![row("X-Team", "blue")]));
    }

    #[test]
    fn a_repeated_name_keeps_the_last_value_whatever_its_case() {
        let rows = [row("X-A", "1"), row("X-B", "2"), row("x-a", "3")];
        assert_eq!(validate(&rows), Ok(vec![row("X-B", "2"), row("x-a", "3")]));
    }

    #[test]
    fn incomplete_or_invalid_rows_name_the_row_to_fix() {
        assert_eq!(
            validate(&[row("", ""), row("", "v")]),
            Err("Header 2 needs a name.".into())
        );
        assert_eq!(
            validate(&[row("bad name", "v")]),
            Err("Header 1 has an invalid name.".into())
        );
        assert_eq!(
            validate(&[row("X-A", "")]),
            Err("Header 1 needs a value.".into())
        );
        assert_eq!(
            validate(&[row("X-A", "one\ntwo")]),
            Err("Header 1 has an invalid value.".into())
        );
    }

    #[test]
    fn the_header_map_skips_entries_that_cannot_be_sent() {
        let map = header_map(&[row("CF-Access-Client-Id", "id"), row("bad name", "v")]);
        assert_eq!(map.len(), 1);
        assert_eq!(map["cf-access-client-id"], "id");
    }
}
