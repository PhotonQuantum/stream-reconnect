use std::future::Future;
use std::io;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::{ready, Sink, Stream};
use log::{error, info};
use tokio::time::sleep;

use crate::config::ReconnectOptions;
use std::error::Error;

/// Trait that should be implemented for an [AsyncRead] and/or [AsyncWrite]
/// item to enable it to work with the [StubbornIo] struct.
pub trait UnderlyingIo<C, E>: Sized + Unpin
where
    C: Clone + Send + Unpin,
    E: Error,
{
    /// The creation function is used by StubbornIo in order to establish both the initial IO connection
    /// in addition to performing reconnects.
    fn establish(ctor_arg: C) -> Pin<Box<dyn Future<Output = Result<Self, E>> + Send>>;

    /// When IO items experience an [io::Error](io::Error) during operation, it does not necessarily mean
    /// it is a disconnect/termination (ex: WouldBlock). This trait provides sensible defaults to classify
    /// which errors are considered "disconnects", but this can be overridden based on the user's needs.
    fn is_disconnect_error(&self, err: &E) -> bool;

    fn exhaust_err() -> E;
}

struct AttemptsTracker {
    attempt_num: usize,
    retries_remaining: Box<dyn Iterator<Item = Duration> + Send>,
}

struct ReconnectStatus<T, C, E> {
    attempts_tracker: AttemptsTracker,
    reconnect_attempt: Pin<Box<dyn Future<Output = Result<T, E>> + Send>>,
    _phantom_data: PhantomData<C>,
    _phantom_data_2: PhantomData<E>,
}

impl<T, C, E> ReconnectStatus<T, C, E>
where
    T: UnderlyingIo<C, E>,
    C: Clone + Send + Unpin + 'static,
    E: Error + Unpin,
{
    pub fn new(options: &ReconnectOptions) -> Self {
        ReconnectStatus {
            attempts_tracker: AttemptsTracker {
                attempt_num: 0,
                retries_remaining: (options.retries_to_attempt_fn)(),
            },
            reconnect_attempt: Box::pin(async { unreachable!("Not going to happen") }),
            _phantom_data: PhantomData,
            _phantom_data_2: PhantomData,
        }
    }
}

/// The StubbornIo is a wrapper over a tokio AsyncRead/AsyncWrite item that will automatically
/// invoke the [UnderlyingIo::establish] upon initialization and when a reconnect is needed.
/// Because it implements deref, you are able to invoke all of the original methods on the wrapped IO.
pub struct StubbornIo<T, C, E> {
    status: Status<T, C, E>,
    underlying_io: T,
    options: ReconnectOptions,
    ctor_arg: C,
}

enum Status<T, C, E> {
    Connected,
    Disconnected(ReconnectStatus<T, C, E>),
    FailedAndExhausted, // the way one feels after programming in dynamically typed languages
}

impl<T, C, E> Deref for StubbornIo<T, C, E> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.underlying_io
    }
}

impl<T, C, E> DerefMut for StubbornIo<T, C, E> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.underlying_io
    }
}

impl<T, C, E> StubbornIo<T, C, E>
where
    T: UnderlyingIo<C, E>,
    C: Clone + Send + Unpin + 'static,
    E: Error + Unpin,
{
    /// Connects or creates a handle to the UnderlyingIo item,
    /// using the default reconnect options.
    pub async fn connect(ctor_arg: C) -> Result<Self, E> {
        let options = ReconnectOptions::new();
        Self::connect_with_options(ctor_arg, options).await
    }

    pub async fn connect_with_options(ctor_arg: C, options: ReconnectOptions) -> Result<Self, E> {
        let tcp = match T::establish(ctor_arg.clone()).await {
            Ok(tcp) => {
                info!("Initial connection succeeded.");
                (options.on_connect_callback)();
                tcp
            }
            Err(e) => {
                error!("Initial connection failed due to: {:?}.", e);
                (options.on_connect_fail_callback)();

                if options.exit_if_first_connect_fails {
                    error!("Bailing after initial connection failure.");
                    return Err(e);
                }

                let mut result = Err(e);

                for (i, duration) in (options.retries_to_attempt_fn)().enumerate() {
                    let reconnect_num = i + 1;

                    info!(
                        "Will re-perform initial connect attempt #{} in {:?}.",
                        reconnect_num, duration
                    );

                    sleep(duration).await;

                    info!("Attempting reconnect #{} now.", reconnect_num);

                    match T::establish(ctor_arg.clone()).await {
                        Ok(tcp) => {
                            result = Ok(tcp);
                            (options.on_connect_callback)();
                            info!("Initial connection successfully established.");
                            break;
                        }
                        Err(e) => {
                            (options.on_connect_fail_callback)();
                            result = Err(e);
                        }
                    }
                }

                match result {
                    Ok(tcp) => tcp,
                    Err(e) => {
                        error!("No more re-connect retries remaining. Never able to establish initial connection.");
                        return Err(e);
                    }
                }
            }
        };

        Ok(StubbornIo {
            status: Status::Connected,
            ctor_arg,
            underlying_io: tcp,
            options,
        })
    }

    fn on_disconnect(mut self: Pin<&mut Self>, cx: &mut Context) {
        match &mut self.status {
            // initial disconnect
            Status::Connected => {
                error!("Disconnect occurred");
                (self.options.on_disconnect_callback)();
                self.status = Status::Disconnected(ReconnectStatus::new(&self.options));
            }
            Status::Disconnected(_) => {
                (self.options.on_connect_fail_callback)();
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

            let future_instant = sleep(next_duration);

            reconnect_status.attempts_tracker.attempt_num += 1;
            let cur_num = reconnect_status.attempts_tracker.attempt_num;

            let reconnect_attempt = async move {
                future_instant.await;
                info!("Attempting reconnect #{} now.", cur_num);
                T::establish(ctor_arg).await
            };

            reconnect_status.reconnect_attempt = Box::pin(reconnect_attempt);

            info!(
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
                (self.options.on_connect_callback)();
                self.underlying_io = underlying_io;
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
            Poll::Ready(Err(err)) => self.is_disconnect_error(err),
            _ => false,
        }
    }
}

impl<T, C, I, E> Stream for StubbornIo<T, C, E>
where
    T: UnderlyingIo<C, E> + Stream<Item = I>,
    C: Clone + Send + Unpin + 'static,
    E: Error + Unpin,
{
    type Item = I;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.status {
            Status::Connected => {
                let poll = ready!(Pin::new(&mut self.underlying_io).poll_next(cx));
                if poll.is_none() {
                    self.on_disconnect(cx);
                    Poll::Pending
                } else {
                    Poll::Ready(poll)
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
        self.underlying_io.size_hint()
    }
}

impl<T, C, I, E> Sink<I> for StubbornIo<T, C, E>
where
    T: UnderlyingIo<C, E> + Sink<I, Error = E>,
    C: Clone + Send + Unpin + 'static,
    E: Error + Unpin,
{
    type Error = E;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.status {
            Status::Connected => {
                let poll = Pin::new(&mut self.underlying_io).poll_ready(cx);

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
        Pin::new(&mut self.underlying_io).start_send(item)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.status {
            Status::Connected => {
                let poll = Pin::new(&mut self.underlying_io).poll_flush(cx);

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
                let poll = Pin::new(&mut self.underlying_io).poll_close(cx);
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
