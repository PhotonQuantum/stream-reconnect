use core::error::Error;
use core::future::Future;
use core::iter::once;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::task::{Context, Poll};
use core::time::Duration;

use alloc::boxed::Box;
use futures::future::BoxFuture;
use futures::{ready, FutureExt, Sink, Stream};
use log::{debug, error, info};

use crate::config::ReconnectOptions;

/// Trait that should be implemented for an [Stream] and/or [Sink]
/// item to enable it to work with the [ReconnectStream] struct.
pub trait UnderlyingStream<C, I, E>
where
    C: Clone + Send + Unpin,
    E: Error,
{
    type Stream: Sized + Unpin;

    /// The creation function is used by [ReconnectStream] in order to establish both the initial IO connection
    /// in addition to performing reconnects.
    #[cfg(feature = "not-send")]
    fn establish(ctor_arg: C) -> impl Future<Output = Result<Self::Stream, E>>;

    /// The creation function is used by [ReconnectStream] in order to establish both the initial IO connection
    /// in addition to performing reconnects.
    #[cfg(not(feature = "not-send"))]
    fn establish(ctor_arg: C) -> impl Future<Output = Result<Self::Stream, E>> + Send;

    /// When sink send experience an `Error` during operation, it does not necessarily mean
    /// it is a disconnect/termination (ex: WouldBlock).
    /// You may specify which errors are considered "disconnects" by this method.
    fn is_write_disconnect_error(err: &E) -> bool;

    /// It's common practice for [Stream] implementations that return an `Err`
    /// when there's an error.
    /// You may match the result to tell them apart from normal response.
    /// By default, no response is considered a "disconnect".
    #[allow(unused_variables)]
    fn is_read_disconnect_error(item: &I) -> bool {
        false
    }

    /// This is returned when retry quota exhausted.
    fn exhaust_err() -> E;
}

struct AttemptsTracker {
    attempt_num: usize,
    retries_remaining: Box<dyn Iterator<Item = Duration> + Send>,
}

struct ReconnectStatus<T, C, I, E>
where
    T: UnderlyingStream<C, I, E>,
    C: Clone + Send + Unpin,
    E: Error,
{
    attempts_tracker: AttemptsTracker,
    #[cfg(not(feature = "not-send"))]
    reconnect_attempt: BoxFuture<'static, Result<T::Stream, E>>,
    #[cfg(feature = "not-send")]
    reconnect_attempt: LocalBoxFuture<'static, Result<T::Stream, E>>,
    _marker: PhantomData<(C, I, E)>,
}

impl<T, C, I, E> ReconnectStatus<T, C, I, E>
where
    T: UnderlyingStream<C, I, E>,
    C: Clone + Send + Unpin + 'static,
    E: Error + Unpin,
{
    pub fn new(options: &ReconnectOptions) -> Self {
        ReconnectStatus {
            attempts_tracker: AttemptsTracker {
                attempt_num: 0,
                retries_remaining: (options.retries_to_attempt_fn())(),
            },
            reconnect_attempt: async { unreachable!("Not going to happen") }.boxed(),
            _marker: PhantomData,
        }
    }
}

/// The ReconnectStream is a wrapper over a [Stream]/[Sink] item that will automatically
/// invoke the [UnderlyingStream::establish] upon initialization and when a reconnect is needed.
/// Because it implements deref, you are able to invoke all of the original methods on the wrapped stream.
pub struct ReconnectStream<T, C, I, E>
where
    T: UnderlyingStream<C, I, E>,
    C: Clone + Send + Unpin,
    E: Error,
{
    status: Status<T, C, I, E>,
    stream: T::Stream,
    options: ReconnectOptions,
    ctor_arg: C,
}

enum Status<T, C, I, E>
where
    T: UnderlyingStream<C, I, E>,
    C: Clone + Send + Unpin,
    E: Error,
{
    Connected,
    Disconnected(ReconnectStatus<T, C, I, E>),
    FailedAndExhausted, // the way one feels after programming in dynamically typed languages
}

impl<T, C, I, E> Deref for ReconnectStream<T, C, I, E>
where
    T: UnderlyingStream<C, I, E>,
    C: Clone + Send + Unpin,
    E: Error,
{
    type Target = T::Stream;

    fn deref(&self) -> &Self::Target {
        &self.stream
    }
}

impl<T, C, I, E> DerefMut for ReconnectStream<T, C, I, E>
where
    T: UnderlyingStream<C, I, E>,
    C: Clone + Send + Unpin,
    E: Error,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.stream
    }
}

impl<T, C, I, E> ReconnectStream<T, C, I, E>
where
    T: UnderlyingStream<C, I, E>,
    C: Clone + Send + Unpin + 'static,
    I: Unpin,
    E: Error + Unpin,
{
    /// Connects or creates a handle to the [UnderlyingStream] item,
    /// using the default reconnect options.
    #[cfg(feature = "std")]
    pub async fn connect(ctor_arg: C) -> Result<Self, E> {
        let options = ReconnectOptions::new();
        Self::connect_with_options(ctor_arg, options).await
    }

    pub async fn connect_with_options(ctor_arg: C, options: ReconnectOptions) -> Result<Self, E> {
        let tries = (**options.retries_to_attempt_fn())()
            .map(Some)
            .chain(once(None));
        let mut result = None;
        for (counter, maybe_delay) in tries.enumerate() {
            match T::establish(ctor_arg.clone()).await {
                Ok(inner) => {
                    debug!("Initial connection succeeded.");
                    (options.on_connect_callback())();
                    result = Some(Ok(inner));
                    break;
                }
                Err(e) => {
                    error!("Connection failed due to: {:?}.", e);
                    (options.on_connect_fail_callback())();

                    if options.exit_if_first_connect_fails() {
                        error!("Bailing after initial connection failure.");
                        return Err(e);
                    }

                    result = Some(Err(e));

                    if let Some(delay) = maybe_delay {
                        debug!(
                            "Will re-perform initial connect attempt #{} in {:?}.",
                            counter + 1,
                            delay
                        );

                        options.sleep_provider()(delay).await;

                        debug!("Attempting reconnect #{} now.", counter + 1);
                    }
                }
            }
        }

        match result.unwrap() {
            Ok(stream) => Ok(ReconnectStream {
                status: Status::Connected,
                stream,
                options,
                ctor_arg,
            }),
            Err(e) => {
                error!("No more re-connect retries remaining. Never able to establish initial connection.");
                Err(e)
            }
        }
    }

    fn on_disconnect(mut self: Pin<&mut Self>, cx: &mut Context) {
        let sleep_provider = self.options.sleep_provider();

        match &mut self.status {
            // initial disconnect
            Status::Connected => {
                error!("Disconnect occurred");
                (self.options.on_disconnect_callback())();
                self.status = Status::Disconnected(ReconnectStatus::new(&self.options));
            }
            Status::Disconnected(_) => {
                (self.options.on_connect_fail_callback())();
            }
            Status::FailedAndExhausted => {
                unreachable!("on_disconnect will not occur for already exhausted state.")
            }
        };

        let ctor_arg = self.ctor_arg.clone();

        // this is ensured to be true now
        if let Status::Disconnected(reconnect_status) = &mut self.status {
            let next_duration = match reconnect_status.attempts_tracker.retries_remaining.next() {
                Some(duration) => duration,
                None => {
                    error!("No more re-connect retries remaining. Giving up.");
                    self.status = Status::FailedAndExhausted;
                    cx.waker().wake_by_ref();
                    return;
                }
            };

            let future_instant = sleep_provider(next_duration);

            reconnect_status.attempts_tracker.attempt_num += 1;
            let cur_num = reconnect_status.attempts_tracker.attempt_num;
            reconnect_status.reconnect_attempt = async move {
                future_instant.await;
                debug!("Attempting reconnect #{} now.", cur_num);
                T::establish(ctor_arg).await
            }
            .boxed();

            debug!(
                "Will perform reconnect attempt #{} in {:?}.",
                reconnect_status.attempts_tracker.attempt_num, next_duration
            );

            cx.waker().wake_by_ref();
        }
    }

    fn poll_disconnect(mut self: Pin<&mut Self>, cx: &mut Context) {
        let (attempt, attempt_num) = match &mut self.status {
            Status::Connected => unreachable!(),
            Status::Disconnected(ref mut status) => (
                Pin::new(&mut status.reconnect_attempt),
                status.attempts_tracker.attempt_num,
            ),
            Status::FailedAndExhausted => unreachable!(),
        };

        match attempt.poll(cx) {
            Poll::Ready(Ok(underlying_io)) => {
                info!("Connection re-established");
                cx.waker().wake_by_ref();
                self.status = Status::Connected;
                (self.options.on_connect_callback())();
                self.stream = underlying_io;
            }
            Poll::Ready(Err(err)) => {
                error!("Connection attempt #{} failed: {:?}", attempt_num, err);
                self.on_disconnect(cx);
            }
            Poll::Pending => {}
        }
    }

    fn is_write_disconnect_detected<X>(&self, poll_result: &Poll<Result<X, E>>) -> bool {
        match poll_result {
            Poll::Ready(Err(err)) => T::is_write_disconnect_error(err),
            _ => false,
        }
    }
}

impl<T, C, I, E> Stream for ReconnectStream<T, C, I, E>
where
    T: UnderlyingStream<C, I, E>,
    T::Stream: Stream<Item = I>,
    C: Clone + Send + Unpin + 'static,
    I: Unpin,
    E: Error + Unpin,
{
    type Item = I;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.status {
            Status::Connected => {
                let poll = ready!(Pin::new(&mut self.stream).poll_next(cx));
                if let Some(poll) = poll {
                    if T::is_read_disconnect_error(&poll) {
                        self.on_disconnect(cx);
                        Poll::Pending
                    } else {
                        Poll::Ready(Some(poll))
                    }
                } else {
                    self.on_disconnect(cx);
                    Poll::Pending
                }
            }
            Status::Disconnected(_) => {
                self.poll_disconnect(cx);
                Poll::Pending
            }
            Status::FailedAndExhausted => Poll::Ready(None),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.stream.size_hint()
    }
}

impl<T, C, I, I2, E> Sink<I> for ReconnectStream<T, C, I2, E>
where
    T: UnderlyingStream<C, I2, E>,
    T::Stream: Sink<I, Error = E>,
    C: Clone + Send + Unpin + 'static,
    I2: Unpin,
    E: Error + Unpin,
{
    type Error = E;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.status {
            Status::Connected => {
                let poll = Pin::new(&mut self.stream).poll_ready(cx);

                if self.is_write_disconnect_detected(&poll) {
                    self.on_disconnect(cx);
                    Poll::Pending
                } else {
                    poll
                }
            }
            Status::Disconnected(_) => {
                self.poll_disconnect(cx);
                Poll::Pending
            }
            Status::FailedAndExhausted => Poll::Ready(Err(T::exhaust_err())),
        }
    }

    fn start_send(mut self: Pin<&mut Self>, item: I) -> Result<(), Self::Error> {
        Pin::new(&mut self.stream).start_send(item)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.status {
            Status::Connected => {
                let poll = Pin::new(&mut self.stream).poll_flush(cx);

                if self.is_write_disconnect_detected(&poll) {
                    self.on_disconnect(cx);
                    Poll::Pending
                } else {
                    poll
                }
            }
            Status::Disconnected(_) => {
                self.poll_disconnect(cx);
                Poll::Pending
            }
            Status::FailedAndExhausted => Poll::Ready(Err(T::exhaust_err())),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.status {
            Status::Connected => {
                let poll = Pin::new(&mut self.stream).poll_close(cx);
                if poll.is_ready() {
                    // if completed, we are disconnected whether error or not
                    self.on_disconnect(cx);
                }

                poll
            }
            Status::Disconnected(_) => Poll::Pending,
            Status::FailedAndExhausted => Poll::Ready(Err(T::exhaust_err())),
        }
    }
}
