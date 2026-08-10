//! The one place a call crosses from GTK's single-threaded main loop onto the
//! multi-thread tokio runtime that hosts backend I/O (D-Bus, subprocesses).
//!
//! `fut` runs on `runtime` (so it may block on real syscalls without stalling
//! GTK); its result comes back through an `async_channel` consumed by
//! `glib::spawn_future_local`, which is safe to await from the main loop since
//! it only ever polls a channel receiver.

use std::future::Future;

pub fn spawn<T, F, C>(runtime: &tokio::runtime::Handle, fut: F, on_result: C)
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
    C: FnOnce(T) + 'static,
{
    let (tx, rx) = async_channel::bounded(1);
    runtime.spawn(async move {
        let result = fut.await;
        let _ = tx.send(result).await;
    });
    glib::spawn_future_local(async move {
        if let Ok(result) = rx.recv().await {
            on_result(result);
        }
    });
}

/// Same GTK↔tokio handshake as [`spawn`], but for a backend that produces a
/// *series* of values (the connectivity watch from NetworkManager) instead of
/// a single result.
pub fn spawn_stream<T, F, C>(runtime: &tokio::runtime::Handle, open: F, on_item: C)
where
    T: Send + 'static,
    F: Future<Output = crate::mx::Result<async_channel::Receiver<T>>> + Send + 'static,
    C: Fn(T) + 'static,
{
    let (tx, rx) = async_channel::bounded(1);
    runtime.spawn(async move {
        let _ = tx.send(open.await).await;
    });
    glib::spawn_future_local(async move {
        let items = match rx.recv().await {
            Ok(Ok(items)) => items,
            Ok(Err(e)) => {
                eprintln!("couldn't open a backend stream: {e}");
                return;
            }
            Err(_) => return,
        };
        while let Ok(item) = items.recv().await {
            on_item(item);
        }
    });
}
