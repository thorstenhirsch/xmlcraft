//! The authorization of an HTTP request the HTTP & OpenAPI window or the tool `openapi_call` sends: none, Basic, a bearer token or an
//! API key (in a header or in the query). The secrets live in memory for as long as the request does: they are not `Serialize`, their
//! `Debug` shows `***`, and [`Auth::redact`] takes them out of anything that is to be shown or returned (an error that names the
//! address, a response that echoes them). Nothing here writes a secret to a file, a setting, the history or a document.

use std::fmt;

/// A secret text: never shown by `Debug`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The text. Use it only to build the request.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// Where an API key goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyPlace {
    Header,
    Query,
}

impl KeyPlace {
    pub fn name(self) -> &'static str {
        match self {
            KeyPlace::Header => "header",
            KeyPlace::Query => "query",
        }
    }
}

/// How a request is authorized.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Auth {
    #[default]
    None,
    Basic {
        user: String,
        password: Secret,
    },
    Bearer {
        token: Secret,
    },
    ApiKey {
        place: KeyPlace,
        name: String,
        value: Secret,
    },
}

/// The most characters of a secret or a name that are accepted.
const MAX_FIELD: usize = 8 * 1024;

impl Auth {
    /// The kind, as the tools name it: `none`, `basic`, `bearer` or `api_key`.
    pub fn kind(&self) -> &'static str {
        match self {
            Auth::None => "none",
            Auth::Basic { .. } => "basic",
            Auth::Bearer { .. } => "bearer",
            Auth::ApiKey { .. } => "api_key",
        }
    }

    /// What may be said about the authorization without giving a secret away: the kind and where a key goes and under which name.
    pub fn summary(&self) -> String {
        match self {
            Auth::None => "none".to_owned(),
            Auth::Basic { .. } => "Basic".to_owned(),
            Auth::Bearer { .. } => "Bearer token".to_owned(),
            Auth::ApiKey { place, name, .. } => format!("API key in the {} {name}", place.name()),
        }
    }

    /// Put the authorization in the request: a header (`Authorization`, or the key's), or a query parameter (percent-encoded) appended
    /// to `url`. A header the request already has under that name is replaced. The error says what is wrong with a field.
    pub fn apply(&self, headers: &mut Vec<(String, String)>, url: &mut String) -> Result<(), String> {
        match self {
            Auth::None => Ok(()),
            Auth::Basic { user, password } => {
                check_text("the user name", user)?;
                check_text("the password", password.expose())?;
                if user.contains(':') {
                    return Err("the user name of Basic authorization cannot hold a colon".to_owned());
                }
                let credentials = format!("{user}:{}", password.expose());
                set_header(headers, "Authorization", format!("Basic {}", base64(credentials.as_bytes())));
                Ok(())
            }
            Auth::Bearer { token } => {
                check_text("the token", token.expose())?;
                if token.is_empty() {
                    return Err("the bearer token is empty".to_owned());
                }
                set_header(headers, "Authorization", format!("Bearer {}", token.expose()));
                Ok(())
            }
            Auth::ApiKey { place, name, value } => {
                check_text("the key's name", name)?;
                check_text("the key", value.expose())?;
                if name.trim().is_empty() {
                    return Err("give the name of the API key (the header or query parameter it is sent in)".to_owned());
                }
                match place {
                    KeyPlace::Header => {
                        if name.contains(|c: char| c.is_whitespace() || c == ':') {
                            return Err(format!("{name} is not a header name"));
                        }
                        set_header(headers, name.trim(), value.expose().to_owned());
                    }
                    KeyPlace::Query => {
                        let (base, fragment) = match url.split_once('#') {
                            Some((b, f)) => (b.to_owned(), Some(f.to_owned())),
                            None => (url.clone(), None),
                        };
                        let mut next = base;
                        next.push(if next.contains('?') { '&' } else { '?' });
                        next.push_str(&percent_encode(name.trim()));
                        next.push('=');
                        next.push_str(&percent_encode(value.expose()));
                        if let Some(f) = fragment {
                            next.push('#');
                            next.push_str(&f);
                        }
                        *url = next;
                    }
                }
                Ok(())
            }
        }
    }

    /// The secrets, as they may appear in a request: the text, its percent-encoding and (for Basic) the encoded credentials.
    fn secrets(&self) -> Vec<String> {
        let mut found = Vec::new();
        match self {
            Auth::None => {}
            Auth::Basic { user, password } => {
                found.push(password.expose().to_owned());
                found.push(percent_encode(password.expose()));
                found.push(base64(format!("{user}:{}", password.expose()).as_bytes()));
            }
            Auth::Bearer { token } => found.push(token.expose().to_owned()),
            Auth::ApiKey { value, .. } => {
                found.push(value.expose().to_owned());
                found.push(percent_encode(value.expose()));
            }
        }
        found.retain(|s| !s.is_empty());
        found.sort_by_key(|s| std::cmp::Reverse(s.len()));
        found.dedup();
        found
    }

    /// `text` with every secret of this authorization replaced by `***`: for an error that names the address, a response that echoes
    /// what it was sent.
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for secret in self.secrets() {
            out = out.replace(&secret, "***");
        }
        out
    }
}

/// Replace the header called `name` (in any case) or add it.
fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: String) {
    headers.retain(|(n, _)| !n.trim().eq_ignore_ascii_case(name));
    headers.push((name.to_owned(), value));
}

/// A field of the authorization is one line, not absurdly long.
fn check_text(what: &str, text: &str) -> Result<(), String> {
    if text.contains(['\r', '\n', '\0']) {
        return Err(format!("{what} cannot hold a line break"));
    }
    if text.len() > MAX_FIELD {
        return Err(format!("{what} is longer than {MAX_FIELD} bytes"));
    }
    Ok(())
}

fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Standard base64 with padding (RFC 4648).
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let (a, b, c) = (chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0));
        let n = (u32::from(a) << 16) | (u32::from(b) << 8) | u32::from(c);
        let at = |shift: u32| ALPHABET.get(((n >> shift) & 63) as usize).copied().map_or('=', char::from);
        out.push(at(18));
        out.push(at(12));
        out.push(if chunk.len() > 1 { at(6) } else { '=' });
        out.push(if chunk.len() > 2 { at(0) } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn applied(auth: &Auth, url: &str) -> (Vec<(String, String)>, String) {
        let mut headers = vec![("Accept".to_owned(), "*/*".to_owned())];
        let mut url = url.to_owned();
        auth.apply(&mut headers, &mut url).expect("applies");
        (headers, url)
    }

    #[test]
    fn basic_is_the_base64_of_user_colon_password() {
        // RFC 7617's example.
        let auth = Auth::Basic { user: "Aladdin".into(), password: Secret::new("open sesame") };
        let (headers, url) = applied(&auth, "https://x.test/a");
        assert_eq!(headers, [("Accept".into(), "*/*".into()), ("Authorization".into(), "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==".into())]);
        assert_eq!(url, "https://x.test/a");
        for (text, expected) in
            [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")]
        {
            assert_eq!(base64(text.as_bytes()), expected);
        }
        assert!(Auth::Basic { user: "a:b".into(), password: Secret::new("x") }.apply(&mut Vec::new(), &mut String::new()).is_err());
    }

    #[test]
    fn a_bearer_token_replaces_an_authorization_header_that_was_there() {
        let auth = Auth::Bearer { token: Secret::new("t0k3n") };
        let mut headers = vec![("authorization".to_owned(), "old".to_owned())];
        auth.apply(&mut headers, &mut String::new()).expect("applies");
        assert_eq!(headers, [("Authorization".to_owned(), "Bearer t0k3n".to_owned())]);
        assert!(Auth::Bearer { token: Secret::new("") }.apply(&mut Vec::new(), &mut String::new()).is_err());
        assert!(Auth::Bearer { token: Secret::new("a\r\nX-Evil: 1") }.apply(&mut Vec::new(), &mut String::new()).is_err(), "no header injection");
    }

    #[test]
    fn an_api_key_goes_in_a_header_or_the_query_encoded() {
        let header = Auth::ApiKey { place: KeyPlace::Header, name: "X-Api-Key".into(), value: Secret::new("k 1") };
        assert_eq!(applied(&header, "https://x.test/").0.last(), Some(&("X-Api-Key".to_owned(), "k 1".to_owned())));
        let query = Auth::ApiKey { place: KeyPlace::Query, name: "api key".into(), value: Secret::new("a&b=c") };
        assert_eq!(applied(&query, "https://x.test/p").1, "https://x.test/p?api%20key=a%26b%3Dc");
        assert_eq!(applied(&query, "https://x.test/p?x=1#top").1, "https://x.test/p?x=1&api%20key=a%26b%3Dc#top");
        assert!(
            Auth::ApiKey { place: KeyPlace::Header, name: "bad name".into(), value: Secret::new("v") }
                .apply(&mut Vec::new(), &mut String::new())
                .is_err()
        );
        assert!(
            Auth::ApiKey { place: KeyPlace::Query, name: " ".into(), value: Secret::new("v") }.apply(&mut Vec::new(), &mut String::new()).is_err()
        );
    }

    #[test]
    fn secrets_are_hidden_from_debug_and_taken_out_of_text() {
        let auth = Auth::Basic { user: "u".into(), password: Secret::new("hunter2") };
        assert!(!format!("{auth:?}").contains("hunter2"));
        let encoded = base64(b"u:hunter2");
        let shown = auth.redact(&format!("failed: Authorization: Basic {encoded} and pw hunter2"));
        assert!(!shown.contains("hunter2") && !shown.contains(&encoded), "{shown}");
        let query = Auth::ApiKey { place: KeyPlace::Query, name: "k".into(), value: Secret::new("a b") };
        assert_eq!(query.redact("https://x.test/?k=a%20b: refused"), "https://x.test/?k=***: refused");
        assert_eq!(Auth::None.redact("nothing"), "nothing");
    }
}
