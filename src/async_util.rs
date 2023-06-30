use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

/// A struct of which references can be used as futures that can be manually woken up by another
/// source.
/// It will then yield nothing. After yielding, it must be woken
/// up to yield again. It can be woken up multiple times before it's
/// been polled.
pub struct Wakeup(AtomicBool);

impl Wakeup {
    pub fn new(initially_woken_up: bool) -> Self {
        Self(AtomicBool::new(initially_woken_up))
    }
    pub fn wakeup(&self) {
        self.0.store(true, Ordering::Release);
    }
}

impl Future for &Wakeup {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.0.swap(false, Ordering::AcqRel) {
            true => Poll::Ready(()),
            false => Poll::Pending,
        }
    }
}
