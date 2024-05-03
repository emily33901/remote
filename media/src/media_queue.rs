use std::collections::VecDeque;

pub(crate) struct MediaQueue<E> {
    queue: VecDeque<E>,
}

impl<E> MediaQueue<E> {
    pub(crate) fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }

    pub(crate) fn push_back(&mut self, e: E) {
        self.queue.push_back(e);
    }

    pub(crate) fn pop_front(&mut self) -> E {
        self.queue.pop_front().unwrap()
    }

    pub(crate) fn drain(&mut self) {
        let _ = self.queue.drain(..);
    }
}
