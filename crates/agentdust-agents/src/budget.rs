use std::cell::Cell;
use std::io::{self, Read};

#[derive(Default)]
pub(crate) struct Budget {
    left: Cell<Option<usize>>,
    tripped: Cell<bool>,
}

impl Budget {
    pub(crate) fn arm(&self, bytes: usize) {
        self.left.set(Some(bytes));
    }

    pub(crate) fn disarm(&self) {
        self.left.set(None);
    }

    pub(crate) fn tripped(&self) -> bool {
        self.tripped.get()
    }
}

pub(crate) struct Metered<'a, R> {
    inner: R,
    budget: &'a Budget,
}

impl<'a, R> Metered<'a, R> {
    pub(crate) fn new(inner: R, budget: &'a Budget) -> Self {
        Self { inner, budget }
    }
}

impl<R: Read> Read for Metered<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let Some(left) = self.budget.left.get() else {
            return self.inner.read(buf);
        };
        if left == 0 && !buf.is_empty() {
            self.budget.tripped.set(true);
            return Err(io::Error::other("field exceeds its byte budget"));
        }
        let take = buf.len().min(left);
        let read = self.inner.read(&mut buf[..take])?;
        self.budget.left.set(Some(left - read));
        Ok(read)
    }
}
