//! Bounded Java-compatible ASCII regex subset shared by Pattern and String APIs.
//! Unsupported constructs fail explicitly rather than adopting Rust-only syntax.
pub fn compile(source: &str, full_match: bool) -> Result<regex::Regex, String> {
    if source.len() > 16_384 || !source.is_ascii() {
        return Err("Pattern is limited to 16 KiB ASCII expressions".into());
    }
    let bytes = source.as_bytes();
    let mut translated = String::new();
    let mut i = 0;
    let mut in_class = false;
    let mut dot_all = false;
    let mut modes = vec![];
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\\' {
            if matches!(
                bytes.get(i + 1),
                Some(b'p' | b'P' | b'v' | b'h' | b'R' | b'X')
            ) {
                return Err(
                    "Unicode regex classes and line-break escapes are not qualified".into(),
                );
            }
            translated.push('\\');
            if let Some(&escaped) = bytes.get(i + 1) {
                translated.push(escaped as char);
            }
            i += 2;
            continue;
        }
        if in_class {
            if c == b'['
                || ["&&", "--", "~~"]
                    .iter()
                    .any(|s| source[i..].starts_with(s))
            {
                return Err(
                    "Nested regex character classes and set operations are not qualified".into(),
                );
            }
            if c == b']' {
                in_class = false;
            }
            translated.push(c as char);
            i += 1;
            continue;
        }
        if c == b'(' && bytes.get(i + 1) == Some(&b'?') {
            if ["(?i)", "(?m)", "(?s)"]
                .iter()
                .any(|p| source[i..].starts_with(p))
            {
                if source[i..].starts_with("(?s)") {
                    dot_all = true;
                }
                translated.push_str(&source[i..i + 4]);
                i += 4;
                continue;
            }
            if !source[i..].starts_with("(?:") {
                return Err(
                    "Regex lookaround, named groups and advanced flags are not implemented".into(),
                );
            }
        }
        match c {
            b'[' => in_class = true,
            b'(' => modes.push(dot_all),
            b')' => dot_all = modes.pop().unwrap_or(false),
            _ => {}
        }
        // Java's default dot excludes both ASCII line terminators. Rust's dot
        // excludes LF only. A local class preserves capture numbers and offsets.
        if c == b'.' && !dot_all {
            translated.push_str("[^\\r\\n]");
        } else {
            translated.push(c as char);
        }
        i += 1;
    }
    if full_match {
        translated = format!("\\A(?:{translated})\\z");
    }
    let compiled = regex::RegexBuilder::new(&translated)
        .size_limit(1_048_576)
        .dfa_size_limit(1_048_576)
        .build()
        .map_err(|e| format!("Regex syntax exceeds the supported Java-compatible subset: {e}"))?;
    if compiled.captures_len() > 128 {
        return Err("Regex capture groups exceed host bound of 127".into());
    }
    Ok(compiled)
}

pub fn validate_input(source: &str, input: &str) -> Result<(), String> {
    if input.len() > 1_048_576 || !input.is_ascii() {
        return Err(
            "Regex input is limited to 1 MiB ASCII until Unicode compatibility is qualified".into(),
        );
    }
    if input.contains(['\r', '\n']) {
        let mut escaped = false;
        let mut in_class = false;
        for c in source.chars() {
            if escaped {
                escaped = false;
                continue;
            }
            if c == '\\' {
                escaped = true;
                continue;
            }
            if c == '[' {
                in_class = true;
            }
            if c == ']' {
                in_class = false;
            }
            if !in_class && (c == '^' || c == '$') {
                return Err(
                    "Java regex line-anchor behavior on line-terminated input is not yet qualified"
                        .into(),
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn java_ascii_dots_flags_and_alternatives() {
        assert!(!compile(".", true).unwrap().is_match("\r"));
        assert!(compile("(?s).", true).unwrap().is_match("\r"));
        assert!(!compile("((?s).).", true).unwrap().is_match("\r\r"));
        assert!(compile("a|ab", true).unwrap().is_match("ab"));
        assert_eq!(
            &compile(r"@group (.*)", false)
                .unwrap()
                .captures("@group Tools\r\n")
                .unwrap()[1],
            "Tools"
        );
        assert!(compile("(?:a)(?U).*", false).is_err());
        assert!(validate_input("a$", "a\n").is_err());
        assert!(validate_input(r"@group (.*)", "@group Tools\r\n").is_ok());
    }
}
