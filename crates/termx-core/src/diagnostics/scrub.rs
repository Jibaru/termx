//! Removal of things that look like secrets from text on its way to the log.
//!
//! termx is written never to log input, output, file contents or the values
//! of environment variables. But error messages come from everywhere, and one
//! of them can carry a token: a failed request that quotes its header, a
//! rejected promise whose message is a response body. This is the last line
//! of defence, applied to every message and every text field of every record.
//!
//! It is pattern matching and therefore best effort, in both directions: a
//! secret in a shape that is not listed here gets through, and an innocent
//! string in one of these shapes is replaced. What is replaced:
//!
//! 1. Tokens with a well-known prefix: GitHub (`ghp_`, `gho_`, `ghu_`,
//!    `ghs_`, `ghr_`, `github_pat_`), GitLab (`glpat-`), Slack (`xoxb-` and
//!    its siblings), OpenAI and Anthropic style keys (`sk-`), Stripe
//!    (`sk_live_`, `rk_live_`, `pk_live_` and the `_test_` ones), AWS access
//!    key ids (`AKIA`, `ASIA`), Google API keys (`AIza`), npm (`npm_`) and
//!    Hugging Face (`hf_`).
//! 2. JSON Web Tokens: three dot-separated parts starting with `eyJ`.
//! 3. What follows `Bearer` or `Basic` when it looks like a credential.
//! 4. The value after a name that says it is a secret (`password`, `secret`,
//!    `token`, `api_key`, `authorization`, `cookie`, ...) and `=` or `:`.
//! 5. Any other run of 32 or more letters, digits, `-` and `_` that mixes
//!    letters and digits irregularly, the way keys and hashes do. UUIDs and
//!    the names of termx's own log files are left alone.

use std::borrow::Cow;

/// What a secret is replaced with.
pub const REDACTED: &str = "<redacted>";

/// Prefixes of well-known tokens, with the number of characters that must
/// follow for the word to be taken for one.
const TOKEN_PREFIXES: &[(&str, usize)] = &[
    ("ghp_", 20),
    ("gho_", 20),
    ("ghu_", 20),
    ("ghs_", 20),
    ("ghr_", 20),
    ("github_pat_", 20),
    ("glpat-", 16),
    ("xoxa-", 10),
    ("xoxb-", 10),
    ("xoxp-", 10),
    ("xoxr-", 10),
    ("xoxs-", 10),
    ("sk-", 16),
    ("sk_live_", 12),
    ("sk_test_", 12),
    ("rk_live_", 12),
    ("rk_test_", 12),
    ("pk_live_", 12),
    ("AKIA", 16),
    ("ASIA", 16),
    ("AIza", 30),
    ("npm_", 30),
    ("hf_", 30),
];

/// Names after which any value is a secret.
const SECRET_NAMES: &[&str] = &["password", "passwd", "pwd", "passphrase", "secret"];

/// Names after which a value is a secret if it is given with `=`, is quoted,
/// or looks like a credential.
const CREDENTIAL_NAMES: &[&str] = &[
    "token",
    "apikey",
    "accesskey",
    "secretkey",
    "privatekey",
    "credential",
    "credentials",
    "authorization",
    "auth",
    "cookie",
    "sessionid",
    "signature",
];

/// Shortest run that is judged by how random it looks.
const MIN_RANDOM_LENGTH: usize = 32;
/// Changes between lower case, upper case and digits that make a run look
/// random rather than like a name.
const MIN_RANDOM_CHANGES: usize = 10;

/// Returns `input` with everything that looks like a secret replaced by
/// [`REDACTED`].
#[must_use]
pub fn secrets(input: &str) -> Cow<'_, str> {
    let bytes = input.as_bytes();
    let mut output: Option<String> = None;
    let mut copied = 0;
    let mut index = 0;

    while index < bytes.len() {
        if !is_word(bytes[index]) || (index > 0 && is_word(bytes[index - 1])) {
            index += 1;
            continue;
        }
        let end = word_end(bytes, index);
        let word = &input[index..end];

        // (start of the secret, its end): everything in between is replaced.
        let secret = if let Some(jwt_end) = jwt_end(bytes, index, end) {
            Some((index, jwt_end))
        } else if has_token_prefix(word) || looks_random(word) {
            Some((index, end))
        } else if word.eq_ignore_ascii_case("bearer") || word.eq_ignore_ascii_case("basic") {
            credential_after_scheme(bytes, end)
        } else {
            value_after_name(bytes, word, end)
        };

        if let Some((start, secret_end)) = secret {
            let output = output.get_or_insert_with(|| String::with_capacity(input.len()));
            // Words and values start and end next to ASCII bytes, so these
            // are character boundaries.
            output.push_str(&input[copied..start]);
            output.push_str(REDACTED);
            copied = secret_end;
            index = secret_end;
        } else {
            index = end;
        }
    }

    match output {
        None => Cow::Borrowed(input),
        Some(mut output) => {
            output.push_str(&input[copied..]);
            Cow::Owned(output)
        }
    }
}

/// Letters, digits, `_` and `-`: what tokens are made of.
fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

fn word_end(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < bytes.len() && is_word(bytes[end]) {
        end += 1;
    }
    end
}

fn has_token_prefix(word: &str) -> bool {
    TOKEN_PREFIXES.iter().any(|(prefix, rest)| {
        word.strip_prefix(prefix)
            .is_some_and(|tail| tail.len() >= *rest)
    })
}

/// End of a JSON Web Token whose first part is `bytes[start..end]`.
fn jwt_end(bytes: &[u8], start: usize, end: usize) -> Option<usize> {
    if !bytes[start..end].starts_with(b"eyJ") || end - start < 10 {
        return None;
    }
    let mut position = end;
    for _ in 0..2 {
        if bytes.get(position) != Some(&b'.') {
            return None;
        }
        let part_end = word_end(bytes, position + 1);
        if part_end - position < 5 {
            return None;
        }
        position = part_end;
    }
    Some(position)
}

/// The credential after `Bearer` or `Basic`, when what follows looks like
/// one rather than like the next word of a sentence.
fn credential_after_scheme(bytes: &[u8], scheme_end: usize) -> Option<(usize, usize)> {
    let start = skip(bytes, scheme_end, |byte| byte == b' ');
    if start == scheme_end {
        return None;
    }
    let end = skip(bytes, start, |byte| {
        is_word(byte) || matches!(byte, b'.' | b'~' | b'+' | b'/' | b'=')
    });
    let value = &bytes[start..end];
    let has_digit = value.iter().any(u8::is_ascii_digit);
    (value.len() >= 12 && (has_digit || value.len() >= 24)).then_some((start, end))
}

/// The value given to `name`, when `name` says that it is a secret:
/// `password=...`, `"api_key": "..."`, `token: abc123...`.
fn value_after_name(bytes: &[u8], name: &str, name_end: usize) -> Option<(usize, usize)> {
    let normalised: String = name
        .chars()
        .filter(|character| !matches!(character, '_' | '-'))
        .map(|character| character.to_ascii_lowercase())
        .collect();
    let always = SECRET_NAMES.iter().any(|known| normalised.ends_with(known));
    let credential = CREDENTIAL_NAMES
        .iter()
        .any(|known| normalised.ends_with(known));
    if !always && !credential {
        return None;
    }

    // name, an optional closing quote, `=` or `:`, an optional opening quote.
    let is_quote = |byte: u8| matches!(byte, b'"' | b'\'');
    let mut position = name_end;
    if bytes.get(position).copied().is_some_and(is_quote) {
        position += 1;
    }
    position = skip(bytes, position, |byte| byte == b' ');
    let separator = *bytes.get(position)?;
    if !matches!(separator, b'=' | b':') {
        return None;
    }
    position = skip(bytes, position + 1, |byte| byte == b' ');
    let quote = bytes.get(position).copied().filter(|byte| is_quote(*byte));
    let start = position + usize::from(quote.is_some());

    let end = match quote {
        Some(quote) => skip(bytes, start, |byte| byte != quote && byte != b'\n'),
        None => skip(bytes, start, |byte| {
            !(byte.is_ascii_whitespace()
                || matches!(byte, b'"' | b'\'' | b'&' | b',' | b';' | b')' | b'}' | b']'))
        }),
    };
    let value = &bytes[start..end];
    if value.is_empty() || value.starts_with(b"<") || value == b"~" {
        // Nothing there, or already replaced.
        return None;
    }

    let looks_like_a_credential = value.len() >= 8 && value.iter().any(u8::is_ascii_digit);
    let is_secret = always || separator == b'=' || quote.is_some() || looks_like_a_credential;
    is_secret.then_some((start, end))
}

/// Whether `word` is long and irregular enough to be a key or a hash.
fn looks_random(word: &str) -> bool {
    let bytes = word.as_bytes();
    if bytes.len() < MIN_RANDOM_LENGTH || word.starts_with("termx-") || is_uuid(bytes) {
        return false;
    }
    let has_digit = bytes.iter().any(u8::is_ascii_digit);
    let has_letter = bytes.iter().any(u8::is_ascii_alphabetic);
    if !has_digit || !has_letter {
        return false;
    }
    let class = |byte: &u8| match byte {
        b'0'..=b'9' => 0,
        b'a'..=b'z' => 1,
        b'A'..=b'Z' => 2,
        _ => 3,
    };
    let changes = bytes
        .windows(2)
        .filter(|pair| class(&pair[0]) != class(&pair[1]))
        .count();
    changes >= MIN_RANDOM_CHANGES
}

/// `8-4-4-4-12` hexadecimal digits.
fn is_uuid(bytes: &[u8]) -> bool {
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

/// The first position at or after `start` where `wanted` no longer holds.
fn skip(bytes: &[u8], start: usize, wanted: impl Fn(u8) -> bool) -> usize {
    let mut position = start;
    while position < bytes.len() && wanted(bytes[position]) {
        position += 1;
    }
    position
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::{REDACTED, secrets};

    /// 36 characters, as after a GitHub prefix.
    const TAIL: &str = "16C7e42F292c6912E7710c838347Ae178B4a";

    #[test]
    fn tokens_with_a_well_known_prefix_are_replaced() {
        for token in [
            format!("ghp_{TAIL}"),
            format!("gho_{TAIL}"),
            format!("github_pat_11ABCDEFG0{TAIL}"),
            "glpat-xXxXxXxXxXxXxXxXxXxX".to_owned(),
            "xoxb-2048-3521-abcdefABCDEF".to_owned(),
            "sk-proj-abcdefghijklmnopqrstuvwx".to_owned(),
            "sk-ant-api03-abcdefghijklmnop".to_owned(),
            "sk_live_abcdefghijklmnop".to_owned(),
            "AKIAIOSFODNN7EXAMPLE".to_owned(),
            "AIzaSyA-abcdefghijklmnopqrstuvwxyz01234".to_owned(),
            format!("npm_{TAIL}"),
        ] {
            let text = format!("request failed with {token}, retrying");
            assert_eq!(
                secrets(&text),
                format!("request failed with {REDACTED}, retrying"),
                "{token}"
            );
        }
    }

    #[test]
    fn json_web_tokens_are_replaced_whole() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(secrets(&format!("jwt={jwt};")), format!("jwt={REDACTED};"));
    }

    #[test]
    fn credentials_after_bearer_and_basic_are_replaced() {
        assert_eq!(
            secrets("Authorization: Bearer abc123def456ghi"),
            format!("Authorization: Bearer {REDACTED}")
        );
        assert_eq!(
            secrets("header 'authorization: basic dXNlcjpwYXNzd29yZA==' rejected"),
            format!("header 'authorization: basic {REDACTED}' rejected")
        );
    }

    #[test]
    fn values_of_secret_names_are_replaced() {
        for (text, expected) in [
            ("password=hunter2", "password=<redacted>"),
            ("db_password: hunter2", "db_password: <redacted>"),
            ("PASSWORD = 'correct horse'", "PASSWORD = '<redacted>'"),
            (
                r#"{"api_key":"abcd","user":"x"}"#,
                r#"{"api_key":"<redacted>","user":"x"}"#,
            ),
            (
                "?access_token=abc&page=2",
                "?access_token=<redacted>&page=2",
            ),
            (
                "client-secret=s3cr3t, next",
                "client-secret=<redacted>, next",
            ),
            ("token: a1b2c3d4e5", "token: <redacted>"),
            ("Cookie: sid=abc12345xyz", "Cookie: <redacted>"),
            ("OPENAI_API_KEY=whatever", "OPENAI_API_KEY=<redacted>"),
        ] {
            assert_eq!(secrets(text), expected, "{text}");
        }
    }

    #[test]
    fn long_irregular_strings_are_replaced() {
        for value in [
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
            "dGhpcyBpcyBhIHZlcnkgbG9uZyBiYXNlNjQgc3RyaW5nIDEyMzQ1",
            "wJalrXUtnFEMI_K7MDENG-bPxRfiCYEXAMPLEKEY1234",
        ] {
            assert_eq!(
                secrets(&format!("got {value}.")),
                format!("got {REDACTED}."),
                "{value}"
            );
        }
    }

    #[test]
    fn ordinary_log_text_is_left_alone() {
        for text in [
            "termx started",
            "command failed",
            "cannot open ~/projects/site/config.toml: No such file or directory (os error 2)",
            "   4: termx_core::diagnostics::writer::LogWriter::submit",
            "             at ./crates/termx-core/src/diagnostics/writer.rs:180:13",
            "  12: core::ops::function::FnOnce::call_once{{vtable.shim}}",
            "at /cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.52.0/src/runtime/task/mod.rs:12",
            "2026-11-26T19:04:11.482Z",
            "os_version=Mac OS 27.2.1 webview_version=22625.2.8.11.9",
            "session 123e4567-e89b-12d3-a456-426614174000 closed",
            "wrote termx-20261126T190411Z-5f0c2a9be771-000.log",
            "saved ~/Downloads/termx-diagnostics-20261126T190411Z.zip",
            "unexpected token: expected identifier",
            "the session token expired",
            "Bearer authentication is required",
            "basic usage is described in the manual",
            "password must be at least 8 characters",
            "author=jane keyboard=us max_tokens=5 monkey: banana",
            "TypeError: undefined is not an object (evaluating 'a.b')",
            "at render (tauri://localhost/assets/index-AfLIx1oD.js:1:2345)",
            "a_record_is_one_json_line_with_the_documented_keys_in_order",
            "VeryLongTypeNameWithManyWordsInItForSureAndThenSome",
            "0x0000000104a3c1f4 - std::backtrace_rs::backtrace::libunwind::trace",
            "ñandú → café ✓",
            "",
        ] {
            let scrubbed = secrets(text);
            assert_eq!(scrubbed, text);
            assert!(matches!(scrubbed, Cow::Borrowed(_)), "{text}");
        }
    }

    #[test]
    fn scrubbing_twice_changes_nothing_more() {
        let once = secrets("password=hunter2 token: 'abc' Bearer abc123def456ghi").into_owned();
        assert_eq!(
            once,
            "password=<redacted> token: '<redacted>' Bearer <redacted>"
        );
        assert_eq!(secrets(&once), once);
    }

    #[test]
    fn text_around_a_secret_is_kept_including_non_ascii() {
        assert_eq!(
            secrets(&format!("café → ghp_{TAIL} ← naïve")),
            "café → <redacted> ← naïve"
        );
    }
}
