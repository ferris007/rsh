//! Naming a job on the command line.
//!
//! `fg` with no argument, `fg %1`, `fg %+`, `fg %-`. The forms are small and
//! the defaults matter more than the syntax: a user who types `fg` means the
//! job they were most recently working with, and getting that wrong is worse
//! than not supporting `%1` at all.

use std::fmt;

use crate::job::JobId;

/// Which job a builtin was asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobSpec {
    /// `%1` — by number.
    Id(JobId),
    /// `%%`, `%+`, or no argument at all: the current job.
    Current,
    /// `%-`: the previous job.
    Previous,
}

/// Why a job specifier could not be understood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadJobSpec {
    /// What the user wrote.
    pub text: String,
}

impl fmt::Display for BadJobSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: no such job", self.text)
    }
}

impl std::error::Error for BadJobSpec {}

impl JobSpec {
    /// Parse a specifier, or default to the current job.
    ///
    /// A bare number is accepted as well as `%1`, because `fg 1` is what people
    /// type and refusing it teaches nothing.
    pub fn parse(text: Option<&str>) -> Result<Self, BadJobSpec> {
        let Some(text) = text else {
            return Ok(Self::Current);
        };

        let body = text.strip_prefix('%').unwrap_or(text);

        match body {
            "" | "%" | "+" => Ok(Self::Current),
            "-" => Ok(Self::Previous),
            digits => digits
                .parse::<usize>()
                .map(|n| Self::Id(JobId(n)))
                .map_err(|_| BadJobSpec {
                    text: text.to_owned(),
                }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_argument_means_the_current_job() {
        assert_eq!(JobSpec::parse(None), Ok(JobSpec::Current));
    }

    #[test]
    fn percent_forms_are_understood() {
        assert_eq!(JobSpec::parse(Some("%1")), Ok(JobSpec::Id(JobId(1))));
        assert_eq!(JobSpec::parse(Some("%%")), Ok(JobSpec::Current));
        assert_eq!(JobSpec::parse(Some("%+")), Ok(JobSpec::Current));
        assert_eq!(JobSpec::parse(Some("%-")), Ok(JobSpec::Previous));
    }

    #[test]
    fn a_bare_number_works_too() {
        // `fg 1` is what people type. Refusing it would teach nothing.
        assert_eq!(JobSpec::parse(Some("2")), Ok(JobSpec::Id(JobId(2))));
    }

    #[test]
    fn anything_else_is_reported() {
        assert_eq!(
            JobSpec::parse(Some("%vim")),
            Err(BadJobSpec {
                text: "%vim".to_owned()
            })
        );
    }
}
