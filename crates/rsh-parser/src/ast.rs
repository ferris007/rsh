//! The syntax tree.
//!
//! Phase 1's tree had one node, because the language had one construct. This
//! one has three, and the shape is the roadmap's:
//!
//! ```text
//! Pipeline
//!  ├── Command
//!  │    ├── echo
//!  │    └── hello
//!  │
//!  └── Command
//!       ├── grep
//!       └── hello
//!            │
//!            └── stdout → result.txt
//! ```
//!
//! Note what the tree does *not* record: which file descriptor a redirection
//! will end up touching after `dup2`, whether the file exists, or how many
//! processes the pipeline will need. Those are execution facts. The tree is a
//! faithful record of what the user wrote, and nothing else.

use crate::span::Span;
use crate::word::Word;

/// One or more commands joined by `|`.
///
/// A single command is a pipeline of length one. Making that the normal case
/// rather than a special one means the executor has a single code path, and
/// Phase 4 changes how a pipeline *runs* without changing what it *is*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline {
    commands: Vec<Command>,
    span: Span,
}

impl Pipeline {
    /// Build a pipeline. At least one command is required.
    pub fn new(commands: Vec<Command>, span: Span) -> Self {
        debug_assert!(
            !commands.is_empty(),
            "a pipeline needs at least one command"
        );
        Self { commands, span }
    }

    /// The commands, left to right.
    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    /// Where the pipeline appeared in the input.
    pub fn span(&self) -> Span {
        self.span
    }
}

/// A simple command: words, plus any redirections attached to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    words: Vec<Word>,
    redirects: Vec<Redirect>,
    span: Span,
}

impl Command {
    /// Build a command.
    pub fn new(words: Vec<Word>, redirects: Vec<Redirect>, span: Span) -> Self {
        Self {
            words,
            redirects,
            span,
        }
    }

    /// The command name and its arguments, before expansion.
    ///
    /// This is not `argv`: a single word can expand to several arguments, or to
    /// none. Turning these into `argv` needs an environment, so it happens in
    /// the executor.
    pub fn words(&self) -> &[Word] {
        &self.words
    }

    /// Redirections, in the order written.
    ///
    /// Order is preserved because it changes the meaning: `>out 2>&1` sends
    /// both streams to the file, while `2>&1 >out` sends stderr to wherever
    /// stdout pointed *before* the file was opened. Sorting these would quietly
    /// break a construct people rely on.
    pub fn redirects(&self) -> &[Redirect] {
        &self.redirects
    }

    /// Where the command appeared in the input.
    pub fn span(&self) -> Span {
        self.span
    }
}

/// A single redirection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    /// The descriptor being redirected.
    ///
    /// Filled in from an explicit number (`2>err`) or from the operator's
    /// default: 0 for input, 1 for output.
    fd: i32,
    kind: RedirectKind,
    span: Span,
}

impl Redirect {
    /// Build a redirection.
    pub fn new(fd: i32, kind: RedirectKind, span: Span) -> Self {
        Self { fd, kind, span }
    }

    /// The descriptor being redirected.
    pub fn fd(&self) -> i32 {
        self.fd
    }

    /// What kind of redirection it is.
    pub fn kind(&self) -> &RedirectKind {
        &self.kind
    }

    /// Where the redirection appeared in the input.
    pub fn span(&self) -> Span {
        self.span
    }
}

/// What a redirection does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedirectKind {
    /// `< file`
    Input(Word),
    /// `> file`
    Output(Word),
    /// `>> file`
    Append(Word),
    /// `<& fd`
    DupInput(Word),
    /// `>& fd` — the `1` in `2>&1`.
    ///
    /// The target is a word rather than a number because POSIX allows it to be
    /// one, and because resolving it is an execution-time question.
    DupOutput(Word),
}

impl RedirectKind {
    /// The default descriptor for this operator when none was written.
    pub fn default_fd(&self) -> i32 {
        match self {
            Self::Input(_) | Self::DupInput(_) => 0,
            Self::Output(_) | Self::Append(_) | Self::DupOutput(_) => 1,
        }
    }

    /// The operator as the user wrote it.
    pub fn operator(&self) -> &'static str {
        match self {
            Self::Input(_) => "<",
            Self::Output(_) => ">",
            Self::Append(_) => ">>",
            Self::DupInput(_) => "<&",
            Self::DupOutput(_) => ">&",
        }
    }

    /// The word after the operator.
    pub fn target(&self) -> &Word {
        match self {
            Self::Input(word)
            | Self::Output(word)
            | Self::Append(word)
            | Self::DupInput(word)
            | Self::DupOutput(word) => word,
        }
    }
}
