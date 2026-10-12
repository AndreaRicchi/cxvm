//! Messages on stderr besides progress: `error:`, `warning:` and `note:`, whose `hint:` lines
//! say what to do next, and with `-v` the `debug:` lines.
//!
//! Labels are coloured the way clap colours its own messages, through [`anstream`]: only on a
//! terminal, never with `NO_COLOR`, always with `CLICOLOR_FORCE`, never with `CLICOLOR=0` or
//! `TERM=dumb`.

use std::fmt;
use std::io::Write;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use anstyle::{AnsiColor, Style};

/// What a message is, printed as its label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    Error,
    Warning,
    Note,
    Debug,
}

impl Label {
    pub fn as_str(self) -> &'static str {
        match self {
            Label::Error => "error",
            Label::Warning => "warning",
            Label::Note => "note",
            Label::Debug => "debug",
        }
    }

    pub fn style(self) -> Style {
        match self {
            Label::Error => AnsiColor::Red.on_default().bold(),
            Label::Warning => AnsiColor::Yellow.on_default().bold(),
            Label::Note => AnsiColor::Cyan.on_default().bold(),
            Label::Debug => Style::new().dimmed(),
        }
    }
}

/// Style of the `hint:` that starts a line of a message.
pub const HINT: Style = AnsiColor::Green.on_default().bold();

/// `text` in `style`. The escapes are removed again when written through [`anstream`] where
/// colour is off.
pub fn paint(style: Style, text: &str) -> String {
    format!("{style}{text}{style:#}")
}

/// `text` after `label:`, with the label and the `hint:` that starts any later line styled.
pub fn render(label: Label, text: &str) -> String {
    let mut out = format!("{} ", paint(label.style(), &format!("{}:", label.as_str())));
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let rest = line.trim_start();
        match rest.strip_prefix("hint:") {
            Some(after) if index > 0 => {
                out.push_str(&line[..line.len() - rest.len()]);
                out.push_str(&paint(HINT, "hint:"));
                out.push_str(after);
            }
            _ => out.push_str(line),
        }
    }
    out
}

/// Prints `label: <text>` on stderr.
pub fn print(label: Label, text: impl fmt::Display) {
    let line = render(label, &text.to_string());
    let _ = writeln!(anstream::stderr(), "{line}");
}

pub fn error(text: impl fmt::Display) {
    print(Label::Error, text);
}

pub fn warning(text: impl fmt::Display) {
    print(Label::Warning, text);
}

pub fn note(text: impl fmt::Display) {
    print(Label::Note, text);
}

static VERBOSE: AtomicBool = AtomicBool::new(false);

/// Turns the `debug:` lines on, for `-v/--verbose`. Set once, before a command runs.
pub fn set_verbose(on: bool) {
    VERBOSE.store(on, Ordering::Relaxed);
}

pub fn is_verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

/// Prints `debug: <message>` on stderr with `-v/--verbose`; the arguments are not even
/// formatted otherwise.
macro_rules! debug {
    ($($arg:tt)*) => {
        if $crate::output::is_verbose() {
            $crate::output::print($crate::output::Label::Debug, format_args!($($arg)*));
        }
    };
}
pub(crate) use debug;

/// `command` as typed in a shell: the program and its arguments, quoted where needed.
pub fn command_line(command: &Command) -> String {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|word| {
            let word = word.to_string_lossy();
            let plain = !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c));
            if plain {
                word.into_owned()
            } else {
                format!("'{}'", word.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> String {
        anstream::adapter::strip_str(text).to_string()
    }

    #[test]
    fn labels_and_hints_are_styled_and_the_text_is_kept() {
        let text = "gcc@13 is not installed\n  hint: run `cxvm install gcc@13`";
        let rendered = render(Label::Error, text);
        assert_eq!(plain(&rendered), format!("error: {text}"));
        assert!(rendered.starts_with(&paint(Label::Error.style(), "error:")));
        assert!(
            rendered.ends_with(&format!(
                "\n  {} run `cxvm install gcc@13`",
                paint(HINT, "hint:")
            )),
            "{rendered:?}"
        );
    }

    #[test]
    fn only_a_hint_that_starts_a_later_line_is_styled() {
        let rendered = render(
            Label::Warning,
            "hint: is part of the message\nsee hint: here",
        );
        assert!(!rendered.contains(&paint(HINT, "hint:")), "{rendered:?}");
        assert_eq!(
            plain(&rendered),
            "warning: hint: is part of the message\nsee hint: here"
        );
    }

    #[test]
    fn command_lines_quote_what_a_shell_would_split() {
        let mut command = Command::new("/s/gcc/bin/g++");
        command.args(["-print-file-name=libstdc++.so.6", "a b", "it's", ""]);
        assert_eq!(
            command_line(&command),
            r"/s/gcc/bin/g++ -print-file-name=libstdc++.so.6 'a b' 'it'\''s' ''"
        );
    }
}
