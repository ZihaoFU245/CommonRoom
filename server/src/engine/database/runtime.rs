//! Temporary bridge for synchronous engine callers during the async storage move.
//! New storage operations should await SQLx directly. This runtime never belongs
//! to a connection, so dropping an engine on Tokio cannot drop a nested runtime.
use super::{Error, Result};
use std::{future::Future, sync::OnceLock};

fn runtime() -> Result<&'static tokio::runtime::Runtime> {
    static RUNTIME: OnceLock<std::result::Result<tokio::runtime::Runtime, std::io::Error>> =
        OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("storage-io")
                .enable_all()
                .build()
        })
        .as_ref()
        .map_err(|_| Error::Conversion("Cannot start storage runtime"))
}

pub(super) fn run<F, T>(future: F) -> Result<T>
where
    F: Future<Output = Result<T>> + Send,
    T: Send,
{
    block_on(future)?
}

pub(crate) fn block_on<F>(future: F) -> Result<F::Output>
where
    F: Future + Send,
    F::Output: Send,
{
    let runtime = runtime()?;
    match tokio::runtime::Handle::try_current().map(|handle| handle.runtime_flavor()) {
        // Existing application callers already run on blocking workers. Enter
        // a blocking region rather than starting another OS thread for each
        // query. This also safely hands off a multithreaded async worker.
        Ok(tokio::runtime::RuntimeFlavor::MultiThread) => {
            Ok(tokio::task::block_in_place(|| runtime.block_on(future)))
        }
        // Current-thread runtimes cannot hand off their executor. Keep the
        // compatibility fallback for synchronous test/embedded entry points.
        Ok(_) => Ok(std::thread::scope(|scope| {
            scope
                .spawn(|| runtime.block_on(future))
                .join()
                .map_err(|_| Error::Conversion("Storage bridge worker failed"))
        })?),
        Err(_) => Ok(runtime.block_on(future)),
    }
}

// Legacy callers may drop pool leases outside a Tokio context. Entering the
// storage runtime lets SQLx schedule its own asynchronous return/close task.
// This does not wait for I/O or create a blocking bridge worker.
pub(super) fn with_context<T>(action: impl FnOnce() -> T) -> Result<T> {
    // Always schedule on the persistent runtime. A caller may use a current-
    // thread runtime and synchronously drop its last database owner; scheduling
    // cleanup on that caller would deadlock its shutdown bridge.
    let _guard = runtime()?.enter();
    Ok(action())
}
