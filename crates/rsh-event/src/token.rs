//! Naming a registered source.

/// A caller-chosen name for a registered descriptor.
///
/// The poller hands this back when the source is ready, so the caller can tell
/// stdin from the signal pipe without comparing descriptor numbers — which it
/// could do, but only until a descriptor is closed and its number reused.
///
/// Both `epoll` and `kqueue` carry an opaque `u64` per registration for exactly
/// this purpose. It is the same idea as `mio`'s `Token`, and for the same
/// reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Token(pub u64);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_comparable_and_copyable() {
        let token = Token(7);
        let copy = token;
        assert_eq!(token, copy);
        assert_ne!(token, Token(8));
    }
}
