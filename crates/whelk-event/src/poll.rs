//! The poller.
//!
//! One interface, two implementations. `epoll` and `kqueue` solve the same
//! problem and disagree about almost everything else — how registration works,
//! whether one call can both register and wait, how a timeout is spelled — so
//! the shared surface is deliberately narrow: register a descriptor, wait,
//! receive tokens.
//!
//! Narrow is the point. Every capability either backend has that the other does
//! not is a capability the shell cannot use portably, and a poller that exposed
//! them would push the platform difference into the caller.

use std::os::fd::RawFd;
use std::time::Duration;

use nix::errno::Errno;

use crate::token::Token;

/// A source that has something to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    /// The name given when the source was registered.
    pub token: Token,
    /// Whether there is something to read.
    pub readable: bool,
    /// Whether the far end has gone away.
    ///
    /// Reported separately from `readable` because they are different facts: a
    /// pipe whose writer closed is readable *and* hung up, and reading it will
    /// return zero bytes rather than block.
    pub hangup: bool,
}

/// Waits for readiness on a set of descriptors.
#[derive(Debug)]
pub struct Poller {
    inner: Inner,
}

impl Poller {
    /// A poller with nothing registered.
    pub fn new() -> Result<Self, Errno> {
        Ok(Self {
            inner: Inner::new()?,
        })
    }

    /// Watch a descriptor for readability.
    ///
    /// The poller does not take ownership: it stores a descriptor number, and
    /// the caller must keep the descriptor open for as long as it is
    /// registered. Closing a registered descriptor removes it from `epoll`
    /// silently, which is a memorable way to build a loop that waits forever.
    pub fn watch(&mut self, fd: RawFd, token: Token) -> Result<(), Errno> {
        self.inner.watch(fd, token)
    }

    /// Stop watching a descriptor.
    pub fn unwatch(&mut self, fd: RawFd) -> Result<(), Errno> {
        self.inner.unwatch(fd)
    }

    /// Wait until something is ready, or the timeout expires.
    ///
    /// Returns an empty slice on timeout. `EINTR` is *not* retried: a signal
    /// arriving during the wait is news, and the caller — a shell, which
    /// installed those handlers on purpose — wants the chance to act on it
    /// rather than being put back to sleep.
    pub fn wait(&mut self, timeout: Option<Duration>) -> Result<&[Event], Errno> {
        self.inner.wait(timeout)
    }
}

// ---- epoll -----------------------------------------------------------------

#[cfg(any(target_os = "linux", target_os = "android"))]
mod backend {
    use super::{Event, Token};
    use nix::errno::Errno;
    use nix::sys::epoll::{Epoll, EpollCreateFlags, EpollEvent, EpollFlags, EpollTimeout};
    use std::os::fd::{BorrowedFd, RawFd};
    use std::time::Duration;

    /// How many events one `epoll_wait` may report.
    ///
    /// A shell watches a handful of descriptors; a larger buffer would be
    /// bytes that never get used. `epoll` simply reports the rest next time.
    const CAPACITY: usize = 16;

    #[derive(Debug)]
    pub(super) struct Inner {
        epoll: Epoll,
        buffer: Vec<EpollEvent>,
        ready: Vec<Event>,
    }

    impl Inner {
        pub(super) fn new() -> Result<Self, Errno> {
            Ok(Self {
                epoll: Epoll::new(EpollCreateFlags::EPOLL_CLOEXEC)?,
                buffer: vec![EpollEvent::empty(); CAPACITY],
                ready: Vec::with_capacity(CAPACITY),
            })
        }

        pub(super) fn watch(&mut self, fd: RawFd, token: Token) -> Result<(), Errno> {
            // Level-triggered, which is the default and the right choice here.
            // Edge-triggered would require draining every descriptor completely
            // on every wake — correct, faster under load, and a great deal
            // easier to get wrong for no benefit at a shell's event rate.
            let event = EpollEvent::new(EpollFlags::EPOLLIN, token.0);

            // SAFETY: the descriptor is valid for the duration of the call, and
            // the caller promises to keep it open while registered.
            let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
            self.epoll.add(borrowed, event)
        }

        pub(super) fn unwatch(&mut self, fd: RawFd) -> Result<(), Errno> {
            // SAFETY: as above.
            let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
            self.epoll.delete(borrowed)
        }

        pub(super) fn wait(&mut self, timeout: Option<Duration>) -> Result<&[Event], Errno> {
            let timeout = match timeout {
                Some(duration) => EpollTimeout::try_from(duration).unwrap_or(EpollTimeout::NONE),
                None => EpollTimeout::NONE,
            };

            let count = self.epoll.wait(&mut self.buffer, timeout)?;

            self.ready.clear();
            for event in &self.buffer[..count] {
                let flags = event.events();
                self.ready.push(Event {
                    token: Token(event.data()),
                    readable: flags.contains(EpollFlags::EPOLLIN),
                    hangup: flags.intersects(EpollFlags::EPOLLHUP | EpollFlags::EPOLLERR),
                });
            }

            Ok(&self.ready)
        }
    }
}

// ---- kqueue ----------------------------------------------------------------

#[cfg(not(any(target_os = "linux", target_os = "android")))]
mod backend {
    use super::{Event, Token};
    use nix::errno::Errno;
    use nix::sys::event::{EvFlags, EventFilter, FilterFlag, KEvent, Kqueue};
    use std::os::fd::RawFd;
    use std::time::Duration;

    /// How many events one `kevent` call may report.
    const CAPACITY: usize = 16;

    /// A timeout of zero: return whatever is ready and do not wait.
    const IMMEDIATELY: nix::libc::timespec = nix::libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };

    #[derive(Debug)]
    pub(super) struct Inner {
        kqueue: Kqueue,
        buffer: Vec<KEvent>,
        ready: Vec<Event>,
    }

    /// An empty event, for filling the result buffer.
    fn blank() -> KEvent {
        KEvent::new(
            0,
            EventFilter::EVFILT_READ,
            EvFlags::empty(),
            FilterFlag::empty(),
            0,
            0,
        )
    }

    impl Inner {
        pub(super) fn new() -> Result<Self, Errno> {
            Ok(Self {
                kqueue: Kqueue::new()?,
                buffer: vec![blank(); CAPACITY],
                ready: Vec::with_capacity(CAPACITY),
            })
        }

        pub(super) fn watch(&mut self, fd: RawFd, token: Token) -> Result<(), Errno> {
            // kqueue carries the caller's value in `udata` rather than in a
            // field of its own.
            let change = KEvent::new(
                fd as usize,
                EventFilter::EVFILT_READ,
                EvFlags::EV_ADD | EvFlags::EV_ENABLE,
                FilterFlag::empty(),
                0,
                token.0 as isize,
            );

            // A zero timeout, so this registers and returns rather than
            // waiting: kqueue submits changes through the same call that waits.
            self.kqueue.kevent(&[change], &mut [], Some(IMMEDIATELY))?;
            Ok(())
        }

        pub(super) fn unwatch(&mut self, fd: RawFd) -> Result<(), Errno> {
            let change = KEvent::new(
                fd as usize,
                EventFilter::EVFILT_READ,
                EvFlags::EV_DELETE,
                FilterFlag::empty(),
                0,
                0,
            );

            self.kqueue.kevent(&[change], &mut [], Some(IMMEDIATELY))?;
            Ok(())
        }

        pub(super) fn wait(&mut self, timeout: Option<Duration>) -> Result<&[Event], Errno> {
            // kqueue takes a `timespec` rather than a count of milliseconds,
            // which is the more precise interface and the less convenient one.
            let timeout = timeout.map(|duration| nix::libc::timespec {
                tv_sec: duration.as_secs() as nix::libc::time_t,
                tv_nsec: i64::from(duration.subsec_nanos()) as _,
            });

            let count = self.kqueue.kevent(&[], &mut self.buffer, timeout)?;

            self.ready.clear();
            for event in &self.buffer[..count] {
                self.ready.push(Event {
                    token: Token(event.udata() as u64),
                    readable: matches!(event.filter(), Ok(EventFilter::EVFILT_READ)),
                    // kqueue reports the far end closing as a flag on the read
                    // event rather than as a separate condition, which is the
                    // more honest description of what happened.
                    hangup: event.flags().contains(EvFlags::EV_EOF),
                });
            }

            Ok(&self.ready)
        }
    }
}

use backend::Inner;

#[cfg(test)]
mod tests {
    use super::*;
    use nix::unistd::{pipe, write};
    use std::os::fd::AsRawFd;

    const STDIN_LIKE: Token = Token(1);
    const OTHER: Token = Token(2);

    #[test]
    fn nothing_registered_means_nothing_happens() {
        let mut poller = Poller::new().expect("failed to create a poller");
        let events = poller
            .wait(Some(Duration::from_millis(20)))
            .expect("wait failed");
        assert!(events.is_empty());
    }

    #[test]
    fn a_written_pipe_becomes_readable() {
        let (read, write_end) = pipe().expect("failed to create a pipe");
        let mut poller = Poller::new().expect("failed to create a poller");
        poller
            .watch(read.as_raw_fd(), STDIN_LIKE)
            .expect("failed to watch");

        // Nothing yet.
        assert!(poller
            .wait(Some(Duration::from_millis(20)))
            .unwrap()
            .is_empty());

        write(&write_end, b"x").expect("failed to write");

        let events = poller
            .wait(Some(Duration::from_millis(200)))
            .expect("wait failed");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].token, STDIN_LIKE);
        assert!(events[0].readable);
    }

    #[test]
    fn the_token_says_which_source_it_was() {
        // The whole reason tokens exist: descriptor numbers are reused, and a
        // loop that compared them would eventually dispatch to the wrong place.
        let (first_read, first_write) = pipe().expect("failed to create a pipe");
        let (second_read, second_write) = pipe().expect("failed to create a pipe");

        let mut poller = Poller::new().expect("failed to create a poller");
        poller.watch(first_read.as_raw_fd(), STDIN_LIKE).unwrap();
        poller.watch(second_read.as_raw_fd(), OTHER).unwrap();

        write(&second_write, b"x").expect("failed to write");

        let events = poller.wait(Some(Duration::from_millis(200))).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].token, OTHER);

        drop(first_write);
    }

    #[test]
    fn several_sources_can_be_ready_at_once() {
        let (first_read, first_write) = pipe().unwrap();
        let (second_read, second_write) = pipe().unwrap();

        let mut poller = Poller::new().unwrap();
        poller.watch(first_read.as_raw_fd(), STDIN_LIKE).unwrap();
        poller.watch(second_read.as_raw_fd(), OTHER).unwrap();

        write(&first_write, b"x").unwrap();
        write(&second_write, b"y").unwrap();

        let events = poller.wait(Some(Duration::from_millis(200))).unwrap();
        assert_eq!(events.len(), 2);

        let mut tokens: Vec<Token> = events.iter().map(|event| event.token).collect();
        tokens.sort();
        assert_eq!(tokens, [STDIN_LIKE, OTHER]);
    }

    #[test]
    fn a_closed_writer_is_reported_as_a_hangup() {
        // Distinct from readable: the descriptor will return zero bytes rather
        // than block, and a loop that treated it as ordinary readability would
        // spin on it forever.
        let (read, write_end) = pipe().unwrap();
        let mut poller = Poller::new().unwrap();
        poller.watch(read.as_raw_fd(), STDIN_LIKE).unwrap();

        drop(write_end);

        let events = poller.wait(Some(Duration::from_millis(200))).unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].hangup, "expected a hangup: {:?}", events[0]);
    }

    #[test]
    fn unwatching_stops_the_reports() {
        let (read, write_end) = pipe().unwrap();
        let mut poller = Poller::new().unwrap();
        poller.watch(read.as_raw_fd(), STDIN_LIKE).unwrap();
        poller.unwatch(read.as_raw_fd()).unwrap();

        write(&write_end, b"x").unwrap();

        assert!(poller
            .wait(Some(Duration::from_millis(50)))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn readiness_persists_until_the_data_is_read() {
        // Level-triggered, not edge-triggered: waiting twice without reading
        // reports the same thing twice. Edge triggering would report it once
        // and require the caller to drain completely, which is faster under
        // load and much easier to get wrong.
        let (read, write_end) = pipe().unwrap();
        let mut poller = Poller::new().unwrap();
        poller.watch(read.as_raw_fd(), STDIN_LIKE).unwrap();

        write(&write_end, b"x").unwrap();

        assert_eq!(
            poller.wait(Some(Duration::from_millis(200))).unwrap().len(),
            1
        );
        assert_eq!(
            poller.wait(Some(Duration::from_millis(200))).unwrap().len(),
            1
        );
    }
}
