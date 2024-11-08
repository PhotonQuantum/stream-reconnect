//! Provides options to configure the behavior of reconnect-stream items,
//! specifically related to reconnect behavior.

use crate::strategies::ExpBackoffStrategy;
use alloc::{boxed::Box, sync::Arc};
use core::time::Duration;
use futures::future::BoxFuture;

pub type DurationIterator = Box<dyn Iterator<Item = Duration> + Send + Sync>;

/// User specified options that control the behavior of the [ReconnectStream](crate::ReconnectStream) upon disconnect.
#[derive(Clone)]
pub struct ReconnectOptions(Box<Inner>);

impl ReconnectOptions {
    pub(crate) fn retries_to_attempt_fn(&self) -> &Arc<dyn Fn() -> DurationIterator + Send + Sync> {
        &self.0.retries_to_attempt_fn
    }
    pub(crate) fn exit_if_first_connect_fails(&self) -> bool {
        self.0.exit_if_first_connect_fails
    }
    pub(crate) fn on_connect_callback(&self) -> &Arc<dyn Fn() + Send + Sync> {
        &self.0.on_connect_callback
    }
    pub(crate) fn on_disconnect_callback(&self) -> &Arc<dyn Fn() + Send + Sync> {
        &self.0.on_disconnect_callback
    }
    pub(crate) fn on_connect_fail_callback(&self) -> &Arc<dyn Fn() + Send + Sync> {
        &self.0.on_connect_fail_callback
    }
    pub(crate) fn sleep_provider(
        &self,
    ) -> Arc<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync> {
        self.0.sleep_provider.clone()
    }
}

#[derive(Clone)]
struct Inner {
    retries_to_attempt_fn: Arc<dyn Fn() -> DurationIterator + Send + Sync>,
    exit_if_first_connect_fails: bool,
    on_connect_callback: Arc<dyn Fn() + Send + Sync>,
    on_disconnect_callback: Arc<dyn Fn() + Send + Sync>,
    on_connect_fail_callback: Arc<dyn Fn() + Send + Sync>,
    sleep_provider: Arc<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>,
}

impl ReconnectOptions {
    /// By default, the [ReconnectStream](crate::ReconnectStream) will not try to reconnect if the first connect attempt fails.
    /// By default, the retries iterator waits longer and longer between reconnection attempts,
    /// until it eventually perpetually tries to reconnect every 30 minutes.
    #[allow(clippy::new_without_default)]
    #[cfg(feature = "std")]
    pub fn new() -> Self {
        ReconnectOptions(Box::new(Inner {
            retries_to_attempt_fn: Arc::new(|| Box::new(ExpBackoffStrategy::default().into_iter())),
            exit_if_first_connect_fails: true,
            on_connect_callback: Arc::new(|| {}),
            on_disconnect_callback: Arc::new(|| {}),
            on_connect_fail_callback: Arc::new(|| {}),
            sleep_provider: Arc::new(|duration| {
                Box::pin({
                    #[cfg(feature = "tokio")]
                    {
                        tokio::time::sleep(duration)
                    }
                    #[cfg(feature = "async-std")]
                    {
                        async_std::task::sleep(delay)
                    }
                })
            }),
        }))
    }

    /// By default, the [ReconnectStream](crate::ReconnectStream) will not try to reconnect if the first connect attempt fails.
    /// By default, the retries iterator waits longer and longer between reconnection attempts,
    /// until it eventually perpetually tries to reconnect every 30 minutes.
    #[allow(clippy::new_without_default)]
    pub fn with_sleep_provider(
        sleep_provider: impl Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync + 'static,
    ) -> Self {
        ReconnectOptions(Box::new(Inner {
            retries_to_attempt_fn: Arc::new(|| Box::new(ExpBackoffStrategy::default().into_iter())),
            exit_if_first_connect_fails: true,
            on_connect_callback: Arc::new(|| {}),
            on_disconnect_callback: Arc::new(|| {}),
            on_connect_fail_callback: Arc::new(|| {}),
            sleep_provider: Arc::new(sleep_provider),
        }))
    }

    /// Represents a function that generates an Iterator
    /// to schedule the wait between reconnection attempts.
    /// This method allows the user to provide any function that returns a value
    /// which is convertible into an iterator, such as an actual iterator or a Vec.
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use stream_reconnect::ReconnectOptions;
    ///
    /// // With the below vector, the ReconnectStream item will try to reconnect three times,
    /// // waiting 2 seconds between each attempt. Once all three tries are exhausted,
    /// // it will stop attempting.
    /// let options = ReconnectOptions::new().with_retries_generator(|| {
    ///     vec![
    ///         Duration::from_secs(2),
    ///         Duration::from_secs(2),
    ///         Duration::from_secs(2),
    ///     ]
    /// });
    /// ```
    pub fn with_retries_generator<F, I, IN>(mut self, retries_generator: F) -> Self
    where
        F: 'static + Send + Sync + Fn() -> IN,
        I: 'static + Send + Sync + Iterator<Item = Duration>,
        IN: IntoIterator<IntoIter = I, Item = Duration>,
    {
        self.0.retries_to_attempt_fn = Arc::new(move || Box::new(retries_generator().into_iter()));
        self
    }

    /// If this is set to true, if the initial connect method of the [ReconnectStream](crate::ReconnectStream) item fails,
    /// then no further reconnects will be attempted
    pub fn with_exit_if_first_connect_fails(mut self, value: bool) -> Self {
        self.0.exit_if_first_connect_fails = value;
        self
    }

    /// Invoked when the [ReconnectStream](crate::ReconnectStream) establishes a connection
    pub fn with_on_connect_callback(mut self, cb: impl Fn() + 'static + Send + Sync) -> Self {
        self.0.on_connect_callback = Arc::new(cb);
        self
    }

    /// Invoked when the [ReconnectStream](crate::ReconnectStream) loses its active connection
    pub fn with_on_disconnect_callback(mut self, cb: impl Fn() + 'static + Send + Sync) -> Self {
        self.0.on_disconnect_callback = Arc::new(cb);
        self
    }

    /// Invoked when the [ReconnectStream](crate::ReconnectStream) fails a connection attempt
    pub fn with_on_connect_fail_callback(mut self, cb: impl Fn() + 'static + Send + Sync) -> Self {
        self.0.on_connect_fail_callback = Arc::new(cb);
        self
    }
}
