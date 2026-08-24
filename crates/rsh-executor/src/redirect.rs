//! Preparing redirections.
//!
//! Everything fallible happens here, in the parent, before any process is
//! forked: expanding the target word, opening the file, checking that a
//! descriptor being duplicated is actually open. What survives is a list of
//! `dup2` calls that cannot fail for any reason the user could have prevented.
//!
//! This is the same rule as `PATH` resolution. A missing file is an ordinary
//! error message here; discovered in the child, it would be an exit code and a
//! guess.

use std::fmt;
use std::fs::OpenOptions;
use std::io;
use std::os::fd::OwnedFd;

use nix::errno::Errno;
use rsh_parser::{Command, RedirectKind, Span};
use rsh_process::Redirections;

use crate::expand::{expand_one, Environment};

/// Why a redirection could not be set up.
#[derive(Debug)]
pub enum RedirectError {
    /// The target expanded to something other than exactly one word.
    ///
    /// `> $FILES` with `FILES="a b"` is an error rather than a redirection to
    /// two files, because there is no sensible thing for it to mean.
    Ambiguous { fields: usize, span: Span },

    /// The file could not be opened.
    Open {
        path: String,
        error: io::Error,
        span: Span,
    },

    /// `>&x` where `x` is not a number.
    NotADescriptor { text: String, span: Span },

    /// `>&9` where descriptor 9 is not open.
    DescriptorNotOpen { fd: i32, span: Span },

    /// `>&-`, which closes a descriptor.
    CloseUnsupported { span: Span },
}

impl RedirectError {
    /// The characters at fault.
    pub fn span(&self) -> Span {
        match self {
            Self::Ambiguous { span, .. }
            | Self::Open { span, .. }
            | Self::NotADescriptor { span, .. }
            | Self::DescriptorNotOpen { span, .. }
            | Self::CloseUnsupported { span } => *span,
        }
    }
}

impl fmt::Display for RedirectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ambiguous { fields, .. } => {
                write!(f, "ambiguous redirect: target expanded to {fields} words")
            }
            // `io::Error`'s own Display appends "(os error 13)", which is
            // noise to someone who just wants to know the file is not
            // writable. Every other shell prints the bare strerror text, and
            // matching that keeps `rsh`'s messages greppable alongside theirs.
            Self::Open { path, error, .. } => match error.raw_os_error() {
                Some(code) => write!(f, "{path}: {}", Errno::from_raw(code).desc()),
                None => write!(f, "{path}: {error}"),
            },
            Self::NotADescriptor { text, .. } => {
                write!(f, "`{text}`: not a file descriptor number")
            }
            Self::DescriptorNotOpen { fd, .. } => write!(f, "{fd}: bad file descriptor"),
            Self::CloseUnsupported { .. } => {
                write!(f, "closing a descriptor with `>&-` is not supported yet")
            }
        }
    }
}

impl std::error::Error for RedirectError {}

/// Turn a command's redirections into a list of descriptor changes.
///
/// Files are opened in the order written, because that order is observable:
/// `> f > f` truncates twice, and `> a > b` leaves `a` created but empty.
pub fn plan(command: &Command, env: &dyn Environment) -> Result<Redirections, RedirectError> {
    let mut plan = Redirections::new();

    for redirect in command.redirects() {
        let span = redirect.span();
        let target = redirect.kind().target();

        let text = expand_one(target, env).map_err(|error| RedirectError::Ambiguous {
            fields: error.fields,
            span,
        })?;

        match redirect.kind() {
            RedirectKind::Input(_) => {
                let file = open(OpenOptions::new().read(true), &text, span)?;
                plan.redirect_to_file(redirect.fd(), file);
            }

            RedirectKind::Output(_) => {
                // Truncate, not append. `>` empties an existing file even if
                // the command that follows writes nothing at all — which is why
                // `> file` is the idiomatic way to blank one.
                let file = open(
                    OpenOptions::new().write(true).create(true).truncate(true),
                    &text,
                    span,
                )?;
                plan.redirect_to_file(redirect.fd(), file);
            }

            RedirectKind::Append(_) => {
                // O_APPEND moves to the end before *every* write, atomically.
                // Seeking to the end once at open time would look equivalent
                // and would interleave badly the moment two processes share the
                // file — which is exactly what `cmd >> log &` does.
                let file = open(
                    OpenOptions::new().write(true).create(true).append(true),
                    &text,
                    span,
                )?;
                plan.redirect_to_file(redirect.fd(), file);
            }

            RedirectKind::DupInput(_) | RedirectKind::DupOutput(_) => {
                let source = descriptor(&text, span)?;
                plan.duplicate(redirect.fd(), source);
            }
        }
    }

    Ok(plan)
}

/// Open a redirection target, keeping the path in the error.
fn open(options: &OpenOptions, path: &str, span: Span) -> Result<OwnedFd, RedirectError> {
    // `std::fs` sets close-on-exec on everything it opens, which is what makes
    // the descriptor a program inherits be exactly the redirected one and not
    // also this original. `dup2` clears the flag on the copy it makes.
    options
        .open(path)
        .map(OwnedFd::from)
        .map_err(|error| RedirectError::Open {
            path: path.to_owned(),
            error,
            span,
        })
}

/// Resolve the right-hand side of `>&` to a descriptor number.
fn descriptor(text: &str, span: Span) -> Result<i32, RedirectError> {
    if text == "-" {
        return Err(RedirectError::CloseUnsupported { span });
    }

    let fd = text
        .parse::<i32>()
        .map_err(|_| RedirectError::NotADescriptor {
            text: text.to_owned(),
            span,
        })?;

    // Checked here so `2>&9` is a message rather than a silent failure inside
    // the child, where the only way to report anything is an exit code.
    if !Redirections::is_open(fd) {
        return Err(RedirectError::DescriptorNotOpen { fd, span });
    }

    Ok(fd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expand::MapEnv;
    use rsh_parser::parse;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A scratch directory unique to this process and call, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("rsh-redirect-{}-{n}", std::process::id()));
            fs::create_dir_all(&dir).expect("failed to create scratch dir");
            Self(dir)
        }

        fn path(&self, name: &str) -> String {
            self.0
                .join(name)
                .to_str()
                .expect("scratch path is not UTF-8")
                .to_owned()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Plan the redirections of a single-command line.
    fn plan_line(input: &str, env: &MapEnv) -> Result<Redirections, RedirectError> {
        let pipeline = parse(input)
            .expect("expected input to parse")
            .expect("expected a command");
        plan(&pipeline.commands()[0], env)
    }

    #[test]
    fn a_command_without_redirections_plans_nothing() {
        let plan = plan_line("echo hi", &MapEnv::new()).expect("expected a plan");
        assert!(plan.is_empty());
    }

    #[test]
    fn output_redirection_creates_the_file_during_planning() {
        // Not when the command runs — when the redirection is set up. This is
        // why `> lockfile` works as a command whose only effect is its
        // redirection, and why a failed command still leaves the file behind.
        let scratch = Scratch::new();
        let path = scratch.path("new.txt");
        assert!(!std::path::Path::new(&path).exists());

        let plan =
            plan_line(&format!("echo hi > {path}"), &MapEnv::new()).expect("expected a plan");
        assert!(!plan.is_empty());
        assert!(std::path::Path::new(&path).exists());
    }

    #[test]
    fn output_redirection_truncates() {
        let scratch = Scratch::new();
        let path = scratch.path("existing.txt");
        fs::write(&path, "previous contents").unwrap();

        plan_line(&format!("echo hi > {path}"), &MapEnv::new()).expect("expected a plan");
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
    }

    #[test]
    fn append_redirection_does_not_truncate() {
        let scratch = Scratch::new();
        let path = scratch.path("log.txt");
        fs::write(&path, "kept\n").unwrap();

        plan_line(&format!("echo hi >> {path}"), &MapEnv::new()).expect("expected a plan");
        assert_eq!(fs::read_to_string(&path).unwrap(), "kept\n");
    }

    #[test]
    fn a_missing_input_file_is_reported_before_anything_runs() {
        let scratch = Scratch::new();
        let path = scratch.path("absent.txt");
        let error =
            plan_line(&format!("cat < {path}"), &MapEnv::new()).expect_err("expected error");
        assert!(matches!(error, RedirectError::Open { .. }));
        assert_eq!(
            error.to_string(),
            format!("{path}: No such file or directory")
        );
    }

    #[test]
    fn the_target_is_expanded() {
        let scratch = Scratch::new();
        let path = scratch.path("expanded.txt");
        let env = MapEnv::new().with("OUT", &path);

        plan_line("echo hi > $OUT", &env).expect("expected a plan");
        assert!(std::path::Path::new(&path).exists());
    }

    #[test]
    fn a_target_that_expands_to_several_words_is_ambiguous() {
        // There is no sensible meaning for "redirect to two files", so the
        // shell refuses rather than picking one.
        let env = MapEnv::new().with("FILES", "a b");
        let error = plan_line("echo hi > $FILES", &env).expect_err("expected error");
        assert!(matches!(error, RedirectError::Ambiguous { fields: 2, .. }));
    }

    #[test]
    fn a_target_that_expands_to_nothing_is_ambiguous_too() {
        let error = plan_line("echo hi > $UNSET", &MapEnv::new()).expect_err("expected error");
        assert!(matches!(error, RedirectError::Ambiguous { fields: 0, .. }));
    }

    #[test]
    fn duplicating_a_standard_descriptor_is_fine() {
        let plan = plan_line("echo hi 2>&1", &MapEnv::new()).expect("expected a plan");
        assert!(!plan.is_empty());
    }

    #[test]
    fn duplicating_a_closed_descriptor_is_caught_in_the_parent() {
        // Checked here rather than in the child, where the only way to report
        // it would be an exit code.
        let error = plan_line("echo hi 2>&9", &MapEnv::new()).expect_err("expected error");
        assert!(matches!(
            error,
            RedirectError::DescriptorNotOpen { fd: 9, .. }
        ));
    }

    #[test]
    fn a_non_numeric_duplication_target_is_reported() {
        let error = plan_line("echo hi 2>&stdout", &MapEnv::new()).expect_err("expected error");
        assert!(matches!(error, RedirectError::NotADescriptor { .. }));
    }

    #[test]
    fn closing_a_descriptor_is_not_supported_yet() {
        let error = plan_line("echo hi 2>&-", &MapEnv::new()).expect_err("expected error");
        assert!(matches!(error, RedirectError::CloseUnsupported { .. }));
    }

    #[test]
    fn errors_carry_the_span_of_the_redirection() {
        let error = plan_line("echo hi 2>&9", &MapEnv::new()).expect_err("expected error");
        assert_eq!(error.span().slice("echo hi 2>&9"), Some("2>&9"));
    }
}
