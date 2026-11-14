//! Exact compact value backing with lazy copy-on-write clones. Mutation still
//! repacks the surviving elements on unwind; unshared values never clone data.
pub type Values<T> = PackedVec<T>;
pub type Text = Box<str>;
pub type Bytes = Box<[u8]>;
pub type Affine = Box<[f32; 6]>;
pub type Color = Box<super::FloatRecord<4>>;

use std::sync::{Arc, OnceLock};
type CopyValues<T> = fn(&[T]) -> Box<[T]>;
struct Shared<T> {
    values: Box<[T]>,
    copy: OnceLock<CopyValues<T>>,
}
pub struct PackedVec<T>(Arc<Shared<T>>);
fn copy_values<T: Clone>(values: &[T]) -> Box<[T]> {
    values.to_vec().into_boxed_slice()
}
impl<T: Clone> Clone for PackedVec<T> {
    fn clone(&self) -> Self {
        // Only Clone can create a shared owner. Install its type-checked copy
        // capability before publishing that owner, so mutation/consumption can
        // retain the existing generic API even for uniquely owned non-Clone T.
        self.0.copy.get_or_init(|| copy_values::<T>);
        Self(Arc::clone(&self.0))
    }
}
impl<T: std::fmt::Debug> std::fmt::Debug for PackedVec<T> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_tuple("PackedVec").field(&self.0.values).finish()
    }
}
impl<T: PartialEq> PartialEq for PackedVec<T> {
    fn eq(&self, other: &Self) -> bool {
        // Preserve element semantics, including non-reflexive NaN values;
        // pointer aliasing alone must never make unequal values compare equal.
        self.0.values == other.0.values
    }
}
impl<T: Eq> Eq for PackedVec<T> {}
impl<T: std::hash::Hash> std::hash::Hash for PackedVec<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&self.0.values, state);
    }
}
impl<T> PackedVec<T> {
    fn owned(values: Box<[T]>) -> Self {
        Self(Arc::new(Shared {
            values,
            copy: OnceLock::new(),
        }))
    }
    fn unique(&mut self) -> &mut Shared<T> {
        if Arc::get_mut(&mut self.0).is_none() {
            let copy = *self
                .0
                .copy
                .get()
                .expect("shared values have Clone capability");
            // Complete every fallible/user Clone before replacing the source.
            let values = copy(&self.0.values);
            self.0 = Arc::new(Shared {
                values,
                copy: OnceLock::new(),
            });
        }
        Arc::get_mut(&mut self.0).expect("new compact value owner is unique")
    }
    fn into_vec(self) -> Vec<T> {
        match Arc::try_unwrap(self.0) {
            Ok(shared) => shared.values.into_vec(),
            Err(shared) => {
                let copy = *shared
                    .copy
                    .get()
                    .expect("shared values have Clone capability");
                copy(&shared.values).into_vec()
            }
        }
    }
}
impl<T> Default for PackedVec<T> {
    fn default() -> Self {
        Self::owned(Box::default())
    }
}
impl<T> std::ops::Deref for PackedVec<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.0.values
    }
}
impl<T> std::ops::DerefMut for PackedVec<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.unique().values
    }
}
impl<T> From<Vec<T>> for PackedVec<T> {
    fn from(v: Vec<T>) -> Self {
        Self::owned(v.into_boxed_slice())
    }
}
impl<T> From<PackedVec<T>> for Vec<T> {
    fn from(v: PackedVec<T>) -> Self {
        v.into_vec()
    }
}
impl<T> FromIterator<T> for PackedVec<T> {
    fn from_iter<I: IntoIterator<Item = T>>(v: I) -> Self {
        Vec::from_iter(v).into()
    }
}
impl<T> IntoIterator for PackedVec<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.into_vec().into_iter()
    }
}
impl<'a, T> IntoIterator for &'a PackedVec<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.values.iter()
    }
}
impl<'a, T> IntoIterator for &'a mut PackedVec<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.unique().values.iter_mut()
    }
}

struct Repack<'a, T> {
    target: &'a mut Box<[T]>,
    values: Vec<T>,
}
impl<T> Drop for Repack<'_, T> {
    fn drop(&mut self) {
        *self.target = std::mem::take(&mut self.values).into_boxed_slice();
    }
}
impl<T> PackedVec<T> {
    fn with_vec<R>(&mut self, f: impl FnOnce(&mut Vec<T>) -> R) -> R {
        let target = &mut self.unique().values;
        let values = std::mem::take(target).into_vec();
        let mut guard = Repack { target, values };
        f(&mut guard.values)
    }
    pub fn capacity(&self) -> usize {
        self.0.values.len()
    }
    /// This owner's control block and packed elements; allocations owned by
    /// elements are charged separately by the recursive graph estimator.
    pub(crate) fn retained_allocation_bytes(&self) -> usize {
        let header = std::mem::size_of::<Shared<T>>() + 2 * std::mem::size_of::<usize>();
        header.saturating_add(self.capacity().saturating_mul(std::mem::size_of::<T>()))
    }
    pub fn shrink_to_fit(&mut self) {}
    pub fn push(&mut self, value: T) {
        self.with_vec(|v| v.push(value));
    }
    pub fn extend<I: IntoIterator<Item = T>>(&mut self, values: I) {
        self.with_vec(|v| v.extend(values));
    }
    pub fn retain(&mut self, f: impl FnMut(&T) -> bool) {
        self.with_vec(|v| v.retain(f));
    }
    pub fn remove(&mut self, index: usize) -> T {
        self.with_vec(|v| v.remove(index))
    }
    pub fn insert(&mut self, index: usize, value: T) {
        self.with_vec(|v| v.insert(index, value));
    }
    pub fn truncate(&mut self, length: usize) {
        self.with_vec(|v| v.truncate(length));
    }
    pub fn clear(&mut self) {
        self.0 = Box::default();
    }
}

pub(crate) fn text_capacity(text: &Text) -> usize {
    text.len()
}
pub(crate) fn bytes_capacity(bytes: &Bytes) -> usize {
    bytes.len()
}
pub(crate) fn boxed_affine_bytes() -> usize {
    std::mem::size_of::<[f32; 6]>()
}
pub(crate) fn boxed_color_bytes() -> usize {
    std::mem::size_of::<super::FloatRecord<4>>()
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
