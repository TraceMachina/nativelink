use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

pub struct DropGuard<F: Future> {
    future: Option<Pin<Box<F>>>,
}

impl<F: Future> DropGuard<F> {
    pub fn new(future: F) -> Self {
        Self {
            future: Some(Box::pin(future)),
        }
    }
}

impl<F: Future> Future for DropGuard<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(future) = self.future.as_mut() {
            match future.as_mut().poll(cx) {
                Poll::Ready(output) => {
                    self.future = None; // Set future to None after it completes
                    Poll::Ready(output)
                }
                Poll::Pending => Poll::Pending,
            }
        } else {
            panic!("Future already completed");
        }
    }
}

impl<F: Future> Drop for DropGuard<F> {
    fn drop(&mut self) {
        if let Some(future) = self.future.take() {
            // Block on the future to ensure it completes.
            futures::executor::block_on(future);
        }
    }
}
