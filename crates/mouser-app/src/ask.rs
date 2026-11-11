//! Yes/no questions on the terminal. Every binary question Mouser asks
//! defaults to yes: `QUESTION [Y/n] `, where Enter, `y` or `yes` is yes and
//! `n`, `no` or anything else is no. Getting no answer at all (end of input,
//! a read error) is never yes. Multi-choice questions (approvals: once,
//! always, deny) are not yes/no and keep their own prompts.

use std::io::{BufRead, Write};

/// Ask `question` with `[Y/n]` and read one line of `input`.
pub fn yes(question: &str, input: &mut dyn BufRead, out: &mut dyn Write) -> bool {
    let _ = write!(out, "{question} [Y/n] ");
    let _ = out.flush();
    let mut s = String::new();
    match input.read_line(&mut s) {
        Ok(0) | Err(_) => {
            let _ = writeln!(out);
            false
        }
        Ok(_) => answer_is_yes(&s),
    }
}

/// One typed answer to a `[Y/n]` question: empty (Enter), `y` or `yes`.
pub fn answer_is_yes(s: &str) -> bool {
    let a = s.trim();
    a.is_empty() || a.eq_ignore_ascii_case("y") || a.eq_ignore_ascii_case("yes")
}

#[cfg(test)]
mod tests {
    use super::{answer_is_yes, yes};
    use std::io::{BufRead, Cursor, Read};

    fn ask(input: &str) -> (bool, String) {
        let mut out = Vec::new();
        let a = yes("Install?", &mut Cursor::new(input.as_bytes().to_vec()), &mut out);
        (a, String::from_utf8(out).unwrap())
    }

    #[test]
    fn enter_y_and_yes_are_yes() {
        for a in ["\n", "\r\n", "  \n", "y\n", "Y\n", "yes\n", "YES\n", "y"] {
            let (y, out) = ask(a);
            assert!(y, "{a:?}");
            assert_eq!(out, "Install? [Y/n] ");
        }
    }

    #[test]
    fn n_no_and_anything_else_are_no() {
        for a in ["n\n", "N\n", "no\n", "No\n", "nope\n", "x\n", "yess\n", "0\n"] {
            assert!(!ask(a).0, "{a:?}");
        }
        assert!(!answer_is_yes("no"));
    }

    #[test]
    fn end_of_input_is_no() {
        let (y, out) = ask("");
        assert!(!y, "no input is not consent");
        assert_eq!(out, "Install? [Y/n] \n");
    }

    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("gone"))
        }
    }
    impl BufRead for Broken {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            Err(std::io::Error::other("gone"))
        }
        fn consume(&mut self, _: usize) {}
    }

    #[test]
    fn a_read_error_is_no() {
        assert!(!yes("Install?", &mut Broken, &mut Vec::new()));
    }
}
