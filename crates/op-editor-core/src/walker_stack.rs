//! Small depth cursors live inline; complete unusually deep trees spill to a
//! Vec without recursion, truncation or an arbitrary traversal-depth limit.
pub(crate) struct Stack<T, const INLINE: usize> {
    inline: [Option<T>; INLINE],
    spill: Vec<T>,
    len: usize,
}
impl<T, const INLINE: usize> Stack<T, INLINE> {
    pub(crate) fn new() -> Self {
        Self {
            inline: std::array::from_fn(|_| None),
            spill: Vec::new(),
            len: 0,
        }
    }
    pub(crate) fn push(&mut self, value: T) {
        if self.len < INLINE {
            self.inline[self.len] = Some(value);
        } else {
            self.spill.push(value);
        }
        self.len += 1;
    }
    pub(crate) fn last_mut(&mut self) -> Option<&mut T> {
        if self.len == 0 {
            None
        } else if self.len <= INLINE {
            self.inline[self.len - 1].as_mut()
        } else {
            self.spill.last_mut()
        }
    }
    pub(crate) fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        if self.len < INLINE {
            self.inline[self.len].take()
        } else {
            self.spill.pop()
        }
    }
}
