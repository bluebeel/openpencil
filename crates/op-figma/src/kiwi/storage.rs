//! Exact owned backing for decoded values. Mutation temporarily restores a Vec;
//! the unwind guard repacks the surviving elements without losing ownership.
pub type Values<T> = PackedVec<T>;
pub type Text = Box<str>;
pub type Bytes = Box<[u8]>;
pub type Affine = Box<[f32; 6]>;
pub type Color = Box<super::FloatRecord<4>>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackedVec<T>(Box<[T]>);
impl<T> Default for PackedVec<T> {
    fn default() -> Self {
        Self(Box::default())
    }
}
impl<T> std::ops::Deref for PackedVec<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.0
    }
}
impl<T> std::ops::DerefMut for PackedVec<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.0
    }
}
impl<T> From<Vec<T>> for PackedVec<T> {
    fn from(v: Vec<T>) -> Self {
        Self(v.into_boxed_slice())
    }
}
impl<T> From<PackedVec<T>> for Vec<T> {
    fn from(v: PackedVec<T>) -> Self {
        v.0.into_vec()
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
        self.0.into_vec().into_iter()
    }
}
impl<'a, T> IntoIterator for &'a PackedVec<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}
impl<'a, T> IntoIterator for &'a mut PackedVec<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter_mut()
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
        let values = std::mem::take(&mut self.0).into_vec();
        let mut guard = Repack {
            target: &mut self.0,
            values,
        };
        f(&mut guard.values)
    }
    pub fn capacity(&self) -> usize {
        self.0.len()
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
