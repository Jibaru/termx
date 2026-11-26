//! Telling a repeat of a message from a new one.
//!
//! "request 17 failed" and "request 18 failed" are the same problem. Both the
//! repeat suppression of the log and the rate limit for frontend records
//! compare messages after replacing what varies between occurrences.

/// Longest key kept, in bytes. Messages that only differ after this are
/// treated as the same.
const MAX_KEY_BYTES: usize = 160;

/// Reduces `message` to what identifies the problem.
///
/// Every run of letters and digits that contains a digit (numbers,
/// identifiers, addresses, hashes) becomes `#`, a group of them joined by
/// punctuation (a version, a UUID, a timestamp, an address with a port)
/// becomes a single `#`, letter case and the amount of white space are
/// ignored, and the result is cut at 160 bytes.
///
/// # Examples
///
/// ```
/// use termx_core::diagnostics::message_key;
///
/// assert_eq!(message_key("Request 17 failed"), message_key("request 18  failed"));
/// assert_ne!(message_key("request failed"), message_key("disk full"));
/// ```
#[must_use]
pub fn message_key(message: &str) -> String {
    let mut key = String::with_capacity(message.len().min(MAX_KEY_BYTES));
    let mut characters = message.chars().peekable();

    while let Some(character) = characters.next() {
        if key.len() >= MAX_KEY_BYTES {
            break;
        }
        if character.is_alphanumeric() || character == '_' {
            let mut word = String::new();
            word.extend(character.to_lowercase());
            let mut has_digit = character.is_ascii_digit();
            while let Some(next) = characters.peek().copied() {
                if !(next.is_alphanumeric() || next == '_') {
                    break;
                }
                has_digit |= next.is_ascii_digit();
                word.extend(next.to_lowercase());
                characters.next();
            }
            if !has_digit {
                key.push_str(&word);
            } else if !key.ends_with('#') {
                key.push('#');
            }
        } else if character.is_whitespace() {
            if !key.ends_with(' ') && !key.is_empty() {
                key.push(' ');
            }
        } else if key.ends_with('#')
            && matches!(character, '-' | '.' | ':' | ',' | '/' | 'x')
            && characters
                .peek()
                .is_some_and(|next| next.is_ascii_alphanumeric())
        {
            // Punctuation inside a group of numbers: wait for what follows.
            // If it is another varying word the group stays one `#`.
            let mut rest = characters.clone();
            let mut varies = false;
            while let Some(next) = rest.next_if(|next| next.is_alphanumeric() || *next == '_') {
                varies |= next.is_ascii_digit();
            }
            if !varies {
                key.push(character);
            }
        } else {
            key.push(character);
        }
    }

    while !key.is_char_boundary(key.len().min(MAX_KEY_BYTES)) {
        key.pop();
    }
    key.truncate(MAX_KEY_BYTES);
    key.truncate(key.trim_end().len());
    key
}

#[cfg(test)]
mod tests {
    use super::{MAX_KEY_BYTES, message_key};

    #[test]
    fn numbers_and_identifiers_do_not_make_a_message_new() {
        assert_eq!(message_key("error 1"), "error #");
        assert_eq!(message_key("error 1"), message_key("error 9999"));
        assert_eq!(
            message_key("session 7f3a9c2e closed after 1500ms"),
            message_key("session 00ab12cd closed after 3ms")
        );
        assert_eq!(
            message_key("cannot read 0x7ffee4b2 at 10.0.0.12:8080"),
            message_key("cannot read 0x1000 at 192.168.1.1:443")
        );
        assert_eq!(
            message_key("request 123e4567-e89b-12d3-a456-426614174000 failed"),
            "request # failed"
        );
        assert_eq!(message_key("at 2026-11-26T19:04:11.482Z"), "at #");
    }

    #[test]
    fn case_and_white_space_are_ignored() {
        assert_eq!(message_key("  Disk   FULL\n"), "disk full");
    }

    #[test]
    fn different_problems_keep_different_keys() {
        let keys = [
            message_key("disk full"),
            message_key("render loop"),
            message_key("x is not a function"),
            message_key("TypeError: y is not a function"),
            message_key("cannot open ~/a.txt"),
            message_key("cannot open ~/b.txt"),
        ];
        for (index, key) in keys.iter().enumerate() {
            assert!(!keys[index + 1..].contains(key), "{key:?} is not unique");
        }
    }

    #[test]
    fn punctuation_that_is_not_inside_a_number_is_kept() {
        assert_eq!(
            message_key("exit code 3: not found"),
            "exit code #: not found"
        );
        assert_eq!(message_key("v2.x is old"), "#.x is old");
    }

    #[test]
    fn long_and_non_ascii_messages_are_cut_safely() {
        let key = message_key(&"é".repeat(1_000));
        assert!(key.len() <= MAX_KEY_BYTES);
        assert!(key.chars().all(|character| character == 'é'));
        assert_eq!(message_key(""), "");
    }
}
