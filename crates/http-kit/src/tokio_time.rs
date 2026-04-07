use std::future::{Future, poll_fn};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MissingTimeDriver;

pub(crate) async fn timeout<F>(
    duration: Duration,
    future: F,
) -> Result<Result<F::Output, tokio::time::error::Elapsed>, MissingTimeDriver>
where
    F: Future,
{
    let future = catch_unwind(AssertUnwindSafe(|| tokio::time::timeout(duration, future)))
        .map_err(|_| MissingTimeDriver)?;
    catch_unwind_future(future)
        .await
        .map_err(|()| MissingTimeDriver)
}

async fn catch_unwind_future<F>(future: F) -> Result<F::Output, ()>
where
    F: Future,
{
    let mut future = Box::pin(future);

    poll_fn(
        move |cx| match catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(cx))) {
            Ok(std::task::Poll::Ready(output)) => std::task::Poll::Ready(Ok(output)),
            Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
            Err(_) => std::task::Poll::Ready(Err(())),
        },
    )
    .await
}
