//! Contains the ingredients needed to create wrappers over [Stream](futures::Stream)/[Sink](futures::Sink) items
//! to automatically reconnect upon failures. This is done so that a user can use them without worrying
//! that their application logic will terminate simply due to an event like a temporary network failure.
//!
//! To wrap existing streams, you simply need to implement the [UnderlyingStream] trait.
//! Once implemented, you can construct it easily by creating a [ReconnectStream] type as seen below.
//!
//! This crate supports both `tokio` and `async-std` runtime.
//!
//! *This crate is a fork of [stubborn-io](https://github.com/craftytrickster/stubborn-io).*
//!
//! *Minimum supported rust version: 1.43.1*
//!
//! ### Runtime Support
//!
//! This crate supports both `tokio` and `async-std` runtime.
//!
//! `tokio` support is enabled by default. While used on an `async-std` runtime, change the corresponding dependency in `Cargo.toml` to
//!
//! ``` toml
//! stream-reconnect = { version = "0.3", default-features = false, features = ["async-std"] }
//! ```
//!
//! ## Feature Gates
//!
//! `not-send` - allow the establish function to be non thread-safe.
//!
//! ### Motivations (preserved from stubborn-io)
//! This crate was created because I was working on a service that needed to fetch data from a remote server
//! via a tokio TcpConnection. It normally worked perfectly (as does all of my code ☺), but every time the
//! remote server had a restart or turnaround, my application logic would stop working.
//! **stubborn-io** was born because I did not want to complicate my service's logic with TcpStream
//! reconnect and disconnect handling code. With stubborn-io, I can keep the service exactly the same,
//! knowing that the StubbornTcpStream's sensible defaults will perform reconnects in a way to keep my service running.
//! Once I realized that the implementation could apply to all IO items and not just TcpStream, I made it customizable as
//! seen below.
//!
//! ## Example on how a ReconnectStream item might be created
//! ```rust
//! use stream_reconnect::{UnderlyingStream, ReconnectStream};
//! use std::future::Future;
//! use std::io;
//! use std::pin::Pin;
//! use tokio::net::TcpStream;
//! # use tokio::net::TcpListener;
//! use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
//! use tokio_tungstenite::tungstenite::{Message, error::Error as WsError};
//! use futures::{SinkExt, Stream, Sink};
//! # use futures::StreamExt;
//! # use std::task::{Context, Poll};
//!
//! struct MyWs;
//!
//! # #[cfg(not(feature = "not-send"))]
//! impl UnderlyingStream<String, Result<Message, WsError>, WsError> for MyWs {
//!     type Stream = WebSocketStream<MaybeTlsStream<TcpStream>>;
//!
//!     // Establishes connection.
//!     // Additionally, this will be used when reconnect tries are attempted.
//!     fn establish(addr: String) -> Pin<Box<dyn Future<Output = Result<Self::Stream, WsError>> + Send>> {
//!         Box::pin(async move {
//!             // In this case, we are trying to connect to the WebSocket endpoint
//!             let ws_connection = connect_async(addr).await.unwrap().0;
//!             Ok(ws_connection)
//!         })
//!     }
//!
//!     // The following errors are considered disconnect errors.
//!     fn is_write_disconnect_error(err: &WsError) -> bool {
//!         matches!(
//!                 err,
//!                 WsError::ConnectionClosed
//!                     | WsError::AlreadyClosed
//!                     | WsError::Io(_)
//!                     | WsError::Tls(_)
//!                     | WsError::Protocol(_)
//!             )
//!     }
//!
//!     // If an `Err` is read, then there might be an disconnection.
//!     fn is_read_disconnect_error(item: &Result<Message, WsError>) -> bool {
//!         if let Err(e) = item {
//!             Self::is_write_disconnect_error(e)
//!         } else {
//!             false
//!         }
//!     }
//!
//!     // Return "Exhausted" if all retry attempts are failed.
//!     fn exhaust_err() -> WsError {
//!         WsError::Io(io::Error::new(io::ErrorKind::Other, "Exhausted"))
//!     }
//! }
//!
//! # #[cfg(not(feature = "not-send"))]
//! type ReconnectWs = ReconnectStream<MyWs, String, Result<Message, WsError>, WsError>;
//!
//! # async fn spawn(tx: tokio::sync::oneshot::Sender<()>) {
//! #     let socket = TcpListener::bind("localhost:8000").await.unwrap();
//! #     tx.send(()).unwrap();
//! #     while let Ok((stream, _)) = socket.accept().await {
//! #         tokio::spawn(async move {
//! #             let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
//! #             while let Some(msg) = ws.next().await { //!
//! #                 ws.send(msg.unwrap()).await.unwrap();
//! #             }
//! #         });
//! #     }
//! # }
//! #
//! # #[cfg(not(feature = "not-send"))]
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() {
//! # let (tx, rx) = tokio::sync::oneshot::channel();
//! # let task = tokio::spawn(spawn(tx));
//! # rx.await.unwrap();
//! let mut ws_stream = ReconnectWs::connect(String::from("ws://localhost:8000")).await.unwrap();
//! ws_stream.send("hello world!".into()).await.unwrap();
//! # task.abort();
//! # }
//! #
//! # #[cfg(feature = "not-send")]
//! # fn main() {}
//! ```

#[doc(inline)]
pub use crate::config::ReconnectOptions;
pub use crate::stream::{ReconnectStream, UnderlyingStream};

pub mod config;
pub mod strategies;
mod stream;
