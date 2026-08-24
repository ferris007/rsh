//! Experiment: you call `unshare(CLONE_NEWPID)`. What is your process id now?
//!
//! See `README.md` in this directory for the question and the observation.
//!
//! Linux only. The other experiments in this directory run anywhere Unix does;
//! namespaces are a Linux invention, and pretending otherwise would be the
//! wrong kind of tidy.

#[cfg(not(target_os = "linux"))]
fn main() {
    println!("namespaces are a Linux feature; this experiment does nothing here");
}

#[cfg(target_os = "linux")]
fn main() {
    use nix::sched::{unshare, CloneFlags};
    use nix::sys::wait::waitpid;
    use nix::unistd::{fork, getpid, ForkResult};

    let before = getpid();
    println!("before unshare, this process is pid {before}");

    // A user namespace as well, because everything else here needs privileges
    // and a user namespace is the one an ordinary user may create. Inside it
    // this process is root, which is what makes the PID namespace allowed.
    let flags = CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWPID;

    if let Err(error) = unshare(flags) {
        println!("unshare failed: {error}");
        println!("(unprivileged user namespaces may be disabled on this kernel)");
        return;
    }

    let after = getpid();
    println!("after unshare, this process is pid {after}");
    println!();

    // SAFETY: the child prints and exits. It allocates while formatting, which
    // is safe here only because this program is single-threaded — the usual
    // fork rule, and the reason the shell itself does not do this.
    match unsafe { fork() }.expect("fork failed") {
        ForkResult::Child => {
            let inside = getpid();
            println!("the child, however, is pid {inside}");

            if inside.as_raw() == 1 {
                println!("  — which makes it init for the namespace");
            }

            std::process::exit(0);
        }
        ForkResult::Parent { child } => {
            println!("and the parent sees that same child as pid {child}");
            let _ = waitpid(child, None);
        }
    }
}
