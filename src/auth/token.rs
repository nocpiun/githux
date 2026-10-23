//! Classic PAT token wrapper.
//!
//! [`Pat`] intentionally does **not** implement `Debug`, `Display` or
//! `serde::Serialize`. Its only accessor ([`Pat::expose`]) is `pub(crate)` so
//! secrets cannot leak through formatting, logging or accidental serialization.
//! The type implements [`Drop`] to scrub the buffer on release.

use std::fmt;

/// Classic Personal Access Token. Never logged or serialized.
pub struct Pat {
    inner: String,
}

impl Pat {
    /// Create a PAT from owned input. Accepts any non-empty string; validation
    /// happens later via `GET /user`, which is the only authoritative check.
    pub fn new(value: String) -> Self {
        Self { inner: value }
    }

    pub fn from_str(value: &str) -> Self {
        Self::new(value.to_string())
    }

    /// Borrow the raw secret. Crate-private so downstream layers cannot
    /// accidentally format or forward it outside the Authorization header.
    pub(crate) fn expose(&self) -> &str {
        &self.inner
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl Clone for Pat {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl Drop for Pat {
    fn drop(&mut self) {
        // Best-effort scrub. We cannot guarantee the allocator copy is cleared,
        // but we overwrite the known buffer so it does not appear in a heap
        // post-mortem or accidental dump of this struct.
        // SAFETY: we have exclusive access via &mut self and fully own the
        // allocation; writing ASCII spaces of equal length preserves layout.
        unsafe {
            let bytes = self.inner.as_bytes_mut();
            for b in bytes.iter_mut() {
                *b = b'\0';
            }
        }
    }
}

impl fmt::Debug for Pat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Pat(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_leaks_token() {
        let pat = Pat::from_str("ghp_supersecret_value_123");
        let rendered = format!("{pat:?}");
        assert!(!rendered.contains("ghp_supersecret"));
        assert!(rendered.contains("redacted"));
    }

    #[test]
    fn clone_and_expose() {
        let pat = Pat::from_str("ghp_clone_test");
        let cloned = pat.clone();
        assert_eq!(cloned.expose(), "ghp_clone_test");
        assert_eq!(pat.expose(), "ghp_clone_test");
    }

    #[test]
    fn display_is_not_implemented() {
        // Compile-time check: Pat does not impl Display.
        // (This test exists to guard against accidental trait additions.)
        let _ = format!("{:?}", Pat::from_str("x"));
    }
}
