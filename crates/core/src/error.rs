//! Exit codes, and the typed failure a handler returns to pick one.
//!
//! An agent reads the exit code before it reads the message, so the code says
//! what kind of next step it needs: fix the call, run setup, or give up.
//! Anything that is not a [`Failure`] is exit 1.

use std::fmt;

use crate::secret::redact;

/// The process exit codes, and what each one asks the caller to do.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Exit {
    Ok = 0,
    /// The service said no, or the thing asked for did not succeed.
    Failed = 1,
    /// The call is wrong: an unknown word, a bad value, a missing `--yes`.
    Usage = 2,
    /// Not signed in, no config section, a tool not installed.
    Setup = 3,
    NotFound = 4,
    /// A revision mismatch, or the thing is already in the state asked for.
    Conflict = 5,
    TimedOut = 124,
    /// What a shell reports for SIGINT. Listed so the table is complete: core
    /// installs no handler, and the default one exits with this.
    Interrupted = 130,
}

impl Exit {
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }
}

/// A failure that knows its exit code and, usually, the command to run next.
///
/// Core finds one anywhere in an `anyhow` chain, so a handler can add context
/// on top of it without losing the code.
#[derive(Debug)]
pub struct Failure {
    pub exit: Exit,
    pub message: String,
    pub hint: Option<String>,
}

impl Failure {
    #[must_use]
    pub fn new(exit: Exit, message: impl Into<String>) -> Self {
        Self {
            exit,
            message: message.into(),
            hint: None,
        }
    }

    #[must_use]
    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(Exit::Usage, message)
    }

    #[must_use]
    pub fn setup(message: impl Into<String>) -> Self {
        Self::new(Exit::Setup, message)
    }

    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(Exit::NotFound, message)
    }

    #[must_use]
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(Exit::Conflict, message)
    }

    #[must_use]
    pub fn timed_out(message: impl Into<String>) -> Self {
        Self::new(Exit::TimedOut, message)
    }

    /// The next step, printed on its own `hint:` line.
    #[must_use]
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

/// The exit code, message and hint for any error, all redacted.
///
/// The message is the whole chain, so context a handler added ("reading work
/// item 42") stays in front of the reason.
pub(crate) fn describe(error: &anyhow::Error) -> (Exit, String, Option<String>) {
    let failure = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<Failure>());
    let exit = failure.map_or(Exit::Failed, |failure| failure.exit);
    let hint = failure.and_then(|failure| failure.hint.as_deref().map(redact));
    (exit, redact(&format!("{error:#}")), hint)
}

#[cfg(test)]
mod tests {
    use anyhow::Context;

    use super::*;

    #[test]
    fn a_failure_keeps_its_code_under_context_and_anything_else_is_one() {
        let wrapped = Err::<(), _>(Failure::not_found("no work item 42").hint("list them first"))
            .context("reading work item 42")
            .unwrap_err();
        let (exit, message, hint) = describe(&wrapped);
        assert_eq!(exit, Exit::NotFound);
        assert_eq!(message, "reading work item 42: no work item 42");
        assert_eq!(hint.as_deref(), Some("list them first"));

        let plain = anyhow::anyhow!("socket closed");
        assert_eq!(describe(&plain).0, Exit::Failed);
        assert_eq!(Exit::TimedOut.code(), 124);
        assert_eq!(Exit::Setup.code(), 3);
    }

    #[test]
    fn the_message_and_hint_are_redacted() {
        let error = anyhow::Error::new(
            Failure::setup("refused Authorization: Bearer abc.def.ghi")
                .hint("retry with password=hunter22"),
        );
        let (_, message, hint) = describe(&error);
        assert!(!message.contains("abc.def"), "{message}");
        assert!(!hint.unwrap().contains("hunter22"));
    }
}
