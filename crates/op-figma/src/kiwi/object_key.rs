//! Shared, owned UTF-8 keys with a one-word handle. Keeping the string's
//! length behind the Arc shrinks each (key, FigValue) slot from 48 to 40 bytes.
//! Decoder interning shares both small allocations across schema-key uses.
use std::sync::Arc;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[allow(clippy::redundant_allocation)] // Keep the shared handle thin for a DST key.
pub struct FigKey(Arc<Box<str>>);

impl FigKey {
    /// Nominal heap storage, charged per reference by conservative caches.
    /// The Arc contains two counters and a boxed-str pointer/length pair.
    pub(crate) fn retained_bytes(&self) -> usize {
        2 * std::mem::size_of::<usize>() + std::mem::size_of::<Box<str>>() + self.len()
    }
    #[cfg(test)]
    pub(super) fn ptr_eq(left: &Self, right: &Self) -> bool {
        Arc::ptr_eq(&left.0, &right.0)
    }
}
impl AsRef<str> for FigKey {
    fn as_ref(&self) -> &str {
        self.0.as_ref().as_ref()
    }
}
impl std::ops::Deref for FigKey {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_ref()
    }
}
impl std::borrow::Borrow<str> for FigKey {
    fn borrow(&self) -> &str {
        self.as_ref()
    }
}
impl std::fmt::Debug for FigKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.as_ref(), f)
    }
}
impl std::fmt::Display for FigKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_ref())
    }
}
impl From<&str> for FigKey {
    fn from(value: &str) -> Self {
        Self(Arc::new(value.into()))
    }
}
impl From<String> for FigKey {
    fn from(value: String) -> Self {
        Self(Arc::new(value.into_boxed_str()))
    }
}
impl From<Box<str>> for FigKey {
    fn from(value: Box<str>) -> Self {
        Self(Arc::new(value))
    }
}
impl From<Arc<str>> for FigKey {
    fn from(value: Arc<str>) -> Self {
        Self::from(value.as_ref())
    }
}

impl<'a> From<std::borrow::Cow<'a, str>> for FigKey {
    fn from(value: std::borrow::Cow<'a, str>) -> Self {
        match value {
            std::borrow::Cow::Borrowed(value) => Self::from(value),
            std::borrow::Cow::Owned(value) => Self::from(value),
        }
    }
}

#[cfg(test)]
#[path = "object_key_tests.rs"]
mod tests;
