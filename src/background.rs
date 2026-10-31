//! Work that runs beside the frame loop, and the channel its answer comes
//! back on.
//!
//! A read of the hub (`hub.rs`) and the event stream (`stream.rs`) never
//! run in a system, so the window never waits on the hub. Each is written
//! once, as a future, and [`spawn`] runs it the way the platform allows:
//!
//! - Natively, on a thread of its own, which drives the future with
//!   `block_on`. The native transports block rather than wait, so the
//!   future never yields and runs to its end as a plain function would.
//! - In a browser, as a task on the page's event loop, since a page has no
//!   threads unless it is served with cross-origin isolation (DESIGN.md
//!   §13). The browser's transports wait on its fetch and its WebSocket,
//!   and [`sleep`] on its timer, and never block.
//!
//! Either way the answer comes back over a [`std::sync::mpsc`] channel that
//! a system polls once per update with [`poll`], which never waits; a
//! channel needs no thread to work, so the systems that drain one do not
//! know which platform they run on. The work handed over is `Send` on both,
//! so code that builds for one builds for the other.

use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::time::Duration;

/// Work running in the background, and the channel its answer comes back
/// on. The receiver sits behind a mutex because a resource must be
/// shareable between threads and a bare receiver is not.
pub struct Pending<T> {
    receiver: Mutex<Receiver<T>>,
}

/// Where a piece of background work has got to.
#[derive(Debug, PartialEq, Eq)]
pub enum Poll<T> {
    Waiting,
    Ready(T),
    /// The work ended without an answer: it panicked.
    Lost,
}

/// Runs the future `work` makes in the background: natively on a new
/// thread named `name`.
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn<F>(name: &str, work: impl FnOnce() -> F + Send + 'static) -> Result<(), String>
where
    F: Future<Output = ()> + 'static,
{
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || bevy::tasks::block_on(work()))
        .map(drop)
        .map_err(|e| format!("cannot start a thread to {name}: {e}"))
}

/// Runs the future `work` makes in the background: in a browser as a task
/// on the page's event loop, which cannot fail to start.
#[cfg(target_arch = "wasm32")]
pub fn spawn<F>(_name: &str, work: impl FnOnce() -> F + Send + 'static) -> Result<(), String>
where
    F: Future<Output = ()> + 'static,
{
    wasm_bindgen_futures::spawn_local(work());
    Ok(())
}

/// Runs the future `work` makes in the background, as [`spawn`] does, and
/// gives the way to take its answer.
pub fn in_background<T, F>(
    name: &str,
    work: impl FnOnce() -> F + Send + 'static,
) -> Result<Pending<T>, String>
where
    T: Send + 'static,
    F: Future<Output = T> + 'static,
{
    let (sender, receiver) = mpsc::channel();
    spawn(name, move || {
        let answer = work();
        async move {
            // A receiver dropped meanwhile means nobody wants the answer.
            let _ = sender.send(answer.await);
        }
    })?;
    Ok(Pending {
        receiver: Mutex::new(receiver),
    })
}

/// Takes the answer if it has come, without waiting for it.
pub fn poll<T>(pending: &Pending<T>) -> Poll<T> {
    let Ok(receiver) = pending.receiver.lock() else {
        return Poll::Lost;
    };
    match receiver.try_recv() {
        Ok(answer) => Poll::Ready(answer),
        Err(TryRecvError::Empty) => Poll::Waiting,
        Err(TryRecvError::Disconnected) => Poll::Lost,
    }
}

/// Waits up to `wait` for a value on `receiver`, from background work:
/// natively blocking the thread, as `recv_timeout` does.
#[cfg(not(target_arch = "wasm32"))]
pub async fn recv_timeout<T>(
    receiver: &Receiver<T>,
    wait: Duration,
) -> Result<T, RecvTimeoutError> {
    receiver.recv_timeout(wait)
}

/// Waits up to `wait` for a value on `receiver`, from background work: in a
/// browser looking every [`LOOK_EVERY`] and yielding to the page between,
/// since a blocking wait would stop the page.
#[cfg(target_arch = "wasm32")]
pub async fn recv_timeout<T>(
    receiver: &Receiver<T>,
    wait: Duration,
) -> Result<T, RecvTimeoutError> {
    let until = web_time::Instant::now() + wait;
    loop {
        match receiver.try_recv() {
            Ok(value) => return Ok(value),
            Err(TryRecvError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
            Err(TryRecvError::Empty) => {}
        }
        let left = until.saturating_duration_since(web_time::Instant::now());
        if left.is_zero() {
            return Err(RecvTimeoutError::Timeout);
        }
        sleep(left.min(LOOK_EVERY)).await;
    }
}

/// How often a wait in the browser looks at its channel.
#[cfg(target_arch = "wasm32")]
const LOOK_EVERY: Duration = Duration::from_millis(10);

/// Yields to the page for `duration`, on its timer.
#[cfg(target_arch = "wasm32")]
pub async fn sleep(duration: Duration) {
    let millis = i32::try_from(duration.as_millis()).unwrap_or(i32::MAX);
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let set = web_sys::window().map(|window| {
            window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, millis)
        });
        // With no timer to wait on, the wait ends at once rather than never.
        if !matches!(set, Some(Ok(_))) {
            let _ = resolve.call0(&wasm_bindgen::JsValue::NULL);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for<T>(pending: &Pending<T>) -> Poll<T> {
        (0..400)
            .find_map(|_| match poll(pending) {
                Poll::Waiting => {
                    std::thread::sleep(Duration::from_millis(5));
                    None
                }
                other => Some(other),
            })
            .unwrap()
    }

    #[test]
    fn background_work_answers_once_and_a_panic_reads_as_lost() {
        let pending = in_background("test", || async { 42 }).unwrap();
        assert_eq!(wait_for(&pending), Poll::Ready(42));
        assert_eq!(poll(&pending), Poll::Lost, "the work has ended");
        let lost: Pending<u8> = in_background("test", || async { panic!("on purpose") }).unwrap();
        assert_eq!(wait_for(&lost), Poll::Lost);
    }

    #[test]
    fn a_wait_for_a_value_ends_with_it_with_the_time_or_with_the_sender() {
        let (sender, receiver) = mpsc::channel();
        sender.send(7).unwrap();
        let wait = Duration::from_millis(20);
        assert_eq!(bevy::tasks::block_on(recv_timeout(&receiver, wait)), Ok(7));
        assert_eq!(
            bevy::tasks::block_on(recv_timeout(&receiver, wait)),
            Err(RecvTimeoutError::Timeout)
        );
        drop(sender);
        assert_eq!(
            bevy::tasks::block_on(recv_timeout(&receiver, wait)),
            Err(RecvTimeoutError::Disconnected)
        );
    }
}
