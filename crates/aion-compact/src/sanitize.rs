use std::sync::LazyLock;

use regex::Regex;

static ANSI_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap());

pub fn strip_ansi(text: &str) -> String {
    ANSI_RE.replace_all(text, "").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_color_codes() {
        let input = "\x1b[31mError\x1b[0m: something failed";
        assert_eq!(strip_ansi(input), "Error: something failed");
    }

    #[test]
    fn strip_ansi_bold_and_nested() {
        let input = "\x1b[1m\x1b[32mCompiling\x1b[0m aion-compact v0.1.0";
        assert_eq!(strip_ansi(input), "Compiling aion-compact v0.1.0");
    }

    #[test]
    fn strip_ansi_no_codes_unchanged() {
        let input = "plain text without any codes";
        assert_eq!(strip_ansi(input), input);
    }

    #[test]
    fn strip_ansi_cursor_movement() {
        let input = "\x1b[2K\x1b[1G> prompt";
        assert_eq!(strip_ansi(input), "> prompt");
    }

    #[test]
    fn strip_ansi_empty_input() {
        assert_eq!(strip_ansi(""), "");
    }
}
