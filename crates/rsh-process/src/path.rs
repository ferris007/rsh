//! Finding the program a user named.
//!
//! `execvp` would search `PATH` on our behalf, but it would do it *inside the
//! child*, where the only way to report "no such command" is an exit code.
//! Resolving in the parent turns the common failure into an ordinary `Result`
//! and means the shell never forks a process it is about to throw away.

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

use nix::unistd::{access, AccessFlags};

/// Why a program name could not be turned into a path to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// No `PATH` entry contained an executable with this name.
    NotFound { program: String },
    /// A file was found, but it cannot be executed.
    ///
    /// Kept distinct from `NotFound` because the fixes are different — a
    /// missing `chmod +x` is not a typo — and because POSIX gives them
    /// different exit codes.
    NotExecutable { path: PathBuf },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { program } => write!(f, "{program}: command not found"),
            Self::NotExecutable { path } => {
                write!(f, "{}: permission denied", path.display())
            }
        }
    }
}

impl std::error::Error for ResolveError {}

/// Resolve a command name to an executable path.
///
/// Follows the POSIX search rules:
///
/// * A name containing `/` is a path. It is used as given and `PATH` is never
///   consulted — this is why `./script` runs the script here and not one that
///   happens to share its name somewhere on `PATH`.
/// * Otherwise each `PATH` entry is tried left to right, and the first
///   candidate that exists and is executable wins.
/// * An empty `PATH` entry means the current directory. This is a real POSIX
///   rule and a real security footgun (a `:` at either end of `PATH` silently
///   puts `.` on it). `rsh` implements it, because diverging quietly from the
///   platform is worse than documenting a sharp edge.
pub fn resolve(program: &str, path_var: Option<&OsStr>) -> Result<PathBuf, ResolveError> {
    if program.contains('/') {
        let candidate = PathBuf::from(program);
        return match classify(&candidate) {
            Candidate::Executable => Ok(candidate),
            Candidate::NotExecutable => Err(ResolveError::NotExecutable { path: candidate }),
            Candidate::Missing => Err(ResolveError::NotFound {
                program: program.to_owned(),
            }),
        };
    }

    let path_var = path_var.unwrap_or(OsStr::new(""));

    // Remembered so that a found-but-unusable file produces a better message
    // than "command not found" if nothing else on PATH works out.
    let mut rejected: Option<PathBuf> = None;

    for entry in std::env::split_paths(path_var) {
        let dir = if entry.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            entry
        };
        let candidate = dir.join(program);
        match classify(&candidate) {
            Candidate::Executable => return Ok(candidate),
            Candidate::NotExecutable => rejected.get_or_insert(candidate),
            Candidate::Missing => continue,
        };
    }

    match rejected {
        Some(path) => Err(ResolveError::NotExecutable { path }),
        None => Err(ResolveError::NotFound {
            program: program.to_owned(),
        }),
    }
}

/// What a candidate path on `PATH` turned out to be.
enum Candidate {
    Executable,
    NotExecutable,
    Missing,
}

/// Decide whether a path is something we can hand to `exec`.
///
/// The permission test is `access(X_OK)` rather than an inspection of the mode
/// bits: the kernel's answer accounts for the process's effective ids, ACLs,
/// and mount options such as `noexec`, none of which are visible in `st_mode`.
///
/// Directories are excluded explicitly. A directory on `PATH` with the same
/// name as the command is searchable — `access(X_OK)` says yes — and `exec`
/// would then fail with `EACCES` inside the child, which is exactly the place
/// we are trying not to discover problems.
fn classify(path: &Path) -> Candidate {
    match path.metadata() {
        Err(_) => Candidate::Missing,
        Ok(meta) if meta.is_dir() => Candidate::Missing,
        Ok(_) => match access(path, AccessFlags::X_OK) {
            Ok(()) => Candidate::Executable,
            Err(_) => Candidate::NotExecutable,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A scratch directory unique to this process and call, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("rsh-path-{}-{n}", std::process::id()));
            fs::create_dir_all(&dir).expect("failed to create scratch dir");
            Self(dir)
        }

        fn file(&self, name: &str, mode: u32) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, "#!/bin/sh\nexit 0\n").expect("failed to write scratch file");
            fs::set_permissions(&path, fs::Permissions::from_mode(mode))
                .expect("failed to set scratch permissions");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn path_var(dirs: &[&Path]) -> std::ffi::OsString {
        std::env::join_paths(dirs).expect("failed to join scratch paths")
    }

    #[test]
    fn finds_the_first_executable_on_path() {
        let a = Scratch::new();
        let b = Scratch::new();
        a.file("tool", 0o755);
        b.file("tool", 0o755);

        let found = resolve("tool", Some(&path_var(&[&a.0, &b.0]))).unwrap();
        assert_eq!(found, a.0.join("tool"));
    }

    #[test]
    fn skips_entries_that_do_not_contain_the_program() {
        let empty = Scratch::new();
        let real = Scratch::new();
        real.file("tool", 0o755);

        let found = resolve("tool", Some(&path_var(&[&empty.0, &real.0]))).unwrap();
        assert_eq!(found, real.0.join("tool"));
    }

    #[test]
    fn a_directory_never_shadows_a_real_executable() {
        // `access(X_OK)` says yes to a searchable directory, so a naive check
        // would stop here and then fail with EACCES inside the child.
        let shadow = Scratch::new();
        fs::create_dir(shadow.0.join("tool")).unwrap();
        let real = Scratch::new();
        real.file("tool", 0o755);

        let found = resolve("tool", Some(&path_var(&[&shadow.0, &real.0]))).unwrap();
        assert_eq!(found, real.0.join("tool"));
    }

    #[test]
    fn missing_program_is_not_found() {
        let dir = Scratch::new();
        assert_eq!(
            resolve("definitely-not-here", Some(&path_var(&[&dir.0]))),
            Err(ResolveError::NotFound {
                program: "definitely-not-here".into()
            })
        );
    }

    #[test]
    fn a_non_executable_file_is_reported_as_such() {
        let dir = Scratch::new();
        let path = dir.file("tool", 0o644);
        assert_eq!(
            resolve("tool", Some(&path_var(&[&dir.0]))),
            Err(ResolveError::NotExecutable { path })
        );
    }

    #[test]
    fn an_executable_later_on_path_beats_an_unusable_one() {
        let broken = Scratch::new();
        broken.file("tool", 0o644);
        let real = Scratch::new();
        real.file("tool", 0o755);

        let found = resolve("tool", Some(&path_var(&[&broken.0, &real.0]))).unwrap();
        assert_eq!(found, real.0.join("tool"));
    }

    #[test]
    fn names_containing_a_slash_bypass_path_entirely() {
        let dir = Scratch::new();
        let path = dir.file("tool", 0o755);
        let decoy = Scratch::new();
        decoy.file("tool", 0o755);

        // Even with a PATH that contains a `tool`, the explicit path wins.
        let found = resolve(path.to_str().unwrap(), Some(&path_var(&[&decoy.0]))).unwrap();
        assert_eq!(found, path);
    }

    #[test]
    fn a_missing_explicit_path_is_not_found_not_a_permission_problem() {
        let dir = Scratch::new();
        let missing = dir.0.join("nope");
        let program = missing.to_str().unwrap();
        assert_eq!(
            resolve(program, None),
            Err(ResolveError::NotFound {
                program: program.to_owned()
            })
        );
    }

    #[test]
    fn an_unset_path_finds_nothing_by_bare_name() {
        assert_eq!(
            resolve("sh", None),
            Err(ResolveError::NotFound {
                program: "sh".into()
            })
        );
    }
}
