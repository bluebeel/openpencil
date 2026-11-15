//! Stable FIFO slots: cancellation unlinks one admission without scanning peers.
//! Freed slots are reused, so backing storage is bounded by live pending calls.
const NONE: usize = usize::MAX;

struct Slot<T> {
    previous: usize,
    next: usize,
    value: Option<T>,
}

pub(super) struct Pending<T> {
    slots: Vec<Slot<T>>,
    free: Vec<usize>,
    head: usize,
    tail: usize,
    len: usize,
}

impl<T> Pending<T> {
    pub(super) fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            head: NONE,
            tail: NONE,
            len: 0,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn front(&self) -> Option<&T> {
        self.slots.get(self.head)?.value.as_ref()
    }

    pub(super) fn push_back(&mut self, value: T) -> usize {
        let index = self.free.pop().unwrap_or_else(|| {
            let index = self.slots.len();
            self.slots.push(Slot {
                previous: NONE,
                next: NONE,
                value: None,
            });
            index
        });
        self.slots[index] = Slot {
            previous: self.tail,
            next: NONE,
            value: Some(value),
        };
        if self.tail == NONE {
            self.head = index;
        } else {
            self.slots[self.tail].next = index;
        }
        self.tail = index;
        self.len += 1;
        index
    }

    pub(super) fn pop_front(&mut self) -> Option<T> {
        self.remove(self.head)
    }

    pub(super) fn remove(&mut self, index: usize) -> Option<T> {
        let (value, previous, next) = {
            let slot = self.slots.get_mut(index)?;
            let value = slot.value.take()?;
            (value, slot.previous, slot.next)
        };
        if previous == NONE {
            self.head = next;
        } else {
            self.slots[previous].next = next;
        }
        if next == NONE {
            self.tail = previous;
        } else {
            self.slots[next].previous = previous;
        }
        self.slots[index].previous = NONE;
        self.slots[index].next = NONE;
        self.free.push(index);
        self.len -= 1;
        Some(value)
    }
}
