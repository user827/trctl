// from: https://fullstackmilk.dev/efficiently_escaping_strings_using_cow_in_rust/
use std::borrow::Cow;

pub fn zsh(input: &str) -> Cow<str> {
    // Iterate through the characters, checking if each one needs escaping
    for (i, ch) in input.chars().enumerate() {
        if zsh_escape_char(ch).is_some() {
            // At least one char needs escaping, so we need to return a brand
            // new `String` rather than the original

            let mut escaped_string = String::with_capacity(input.len());
            // Calling `String::with_capacity()` instead of `String::new()` is
            // a slight optimisation to reduce the number of allocations we
            // need to do.
            //
            // We know that the escaped string is always at least as long as
            // the unescaped version so we can preallocate at least that much
            // space.

            // We already checked the characters up to index `i` don't need
            // escaping so we can just copy them straight in
            escaped_string.push_str(&input[..i]);

            // Escape the remaining characters if they need it and add them to
            // our escaped string
            for ch in input[i..].chars() {
                match zsh_escape_char(ch) {
                    Some(escaped_char) => escaped_string.push_str(escaped_char),
                    None => escaped_string.push(ch),
                };
            }

            return Cow::Owned(escaped_string);
        }
    }

    // We've iterated through all of `input` and didn't find any special
    // characters, so it's safe to just return the original string
    Cow::Borrowed(input)
}

fn zsh_escape_char(ch: char) -> Option<&'static str> {
    match ch {
        ':' => Some("\\:"),
        _ => None,
    }
}
