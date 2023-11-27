use async_recursion::async_recursion;
use futures::Future;

use futures::task::SpawnExt;

use hala_reactor::*;

static POOL: std::sync::OnceLock<futures::executor::ThreadPool> = std::sync::OnceLock::new();

pub fn socket_tester<IO, T, Fut>(test: T)
where
    IO: IoDevice + ContextIoDevice + 'static,
    T: FnOnce() -> Fut,
    Fut: Future + Send + 'static,
    Fut::Output: Send,
{
    let thread_pool = spawner::<IO>();

    let handle = if IO::Guard::<()>::is_multithread() {
        log::trace!("start multi-thread io test");

        // IO::get().start(None);

        thread_pool.spawn_with_handle(test()).unwrap()
    } else {
        log::trace!("start single thread io test");

        let future = test();

        thread_pool.spawn(run_event_loop::<IO>()).unwrap();

        thread_pool.spawn_with_handle(future).unwrap()
    };

    futures::executor::block_on(handle);
}

#[async_recursion]
async fn run_event_loop<IO>()
where
    IO: IoDevice + ContextIoDevice + 'static,
{
    IO::get().poll_once(None).unwrap();

    spawner::<IO>().spawn(run_event_loop::<IO>()).unwrap();
}

pub fn spawner<IO>() -> &'static futures::executor::ThreadPool
where
    IO: IoDevice,
{
    POOL.get_or_init(|| {
        #[cfg(feature = "debug")]
        pretty_env_logger::init();

        let pool_size = if IO::Guard::<()>::is_multithread() {
            10
        } else {
            1
        };

        log::info!("hala io tester start with {} threads", pool_size);

        futures::executor::ThreadPool::builder()
            .pool_size(pool_size)
            .create()
            .unwrap()
    })
}

pub use futures;

pub use hala_io_test_derive::*;