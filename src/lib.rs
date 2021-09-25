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
//! ```
//! use stream_reconnect::{UnderlyingStream, ReconnectStream};
//! use std::future::Future;
//! use std::io;
//! use std::pin::Pin;
//! use tokio::net::TcpStream;
//! use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
//! use tokio_tungstenite::tungstenite::{Message, error::Error as WsError};
//! use futures::{SinkExt, Stream, Sink};
//! use std::task::{Context, Poll};
//!
//! struct MyWs(WebSocketStream<MaybeTlsStream<TcpStream>>);
//!
//! // implement Stream & Sink for MyWs
//! # impl Stream for MyWs {
//! #     type Item = Result<Message, WsError>;
//! #
//! #     fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>>{
//! #         Pin::new(&mut self.0).poll_next(cx)
//! #     }
//! # }
//! #
//! # impl Sink<Message> for MyWs {
//! #     type Error = WsError;
//! #
//! #     fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>>{
//! #         Pin::new(&mut self.0).poll_ready(cx)
//! #     }
//! #
//! #     fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error>{
//! #         Pin::new(&mut self.0).start_send(item)
//! #     }
//! #
//! #     fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>>{
//! #         Pin::new(&mut self.0).poll_flush(cx)
//! #     }
//! #
//! #     fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>>{
//! #         Pin::new(&mut self.0).poll_close(cx)
//! #     }
//! # }
//!
//! impl UnderlyingStream<String, Result<Message, WsError>, WsError> for MyWs {
//!     // Establishes connection.
//!     // Additionally, this will be used when reconnect tries are attempted.
//!     fn establish(addr: String) -> Pin<Box<dyn Future<Output = Result<Self, WsError>> + Send>> {
//!         Box::pin(async move {
//!             // In this case, we are trying to connect to the WebSocket endpoint
//!             let ws_connection = connect_async(addr).await.unwrap().0;
//!             Ok(MyWs(ws_connection))
//!         })
//!     }
//!
//!     // The following errors are considered disconnect errors.
//!     fn is_write_disconnect_error(&self, err: &WsError) -> bool {
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
//!     fn is_read_disconnect_error(&self, item: &Result<Message, WsError>) -> bool {
//!         if let Err(e) = item {
//!             self.is_write_disconnect_error(e)
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
//! type ReconnectWs = ReconnectStream<MyWs, String, Result<Message, WsError>, WsError>;
//!
//! # async fn test() {
//! let mut ws_stream: ReconnectWs = ReconnectWs::connect(String::from("wss://localhost:8000")).await.unwrap();
//! ws_stream.send(Message::text(String::from("hello world!"))).await.unwrap();
//! # }
//! ```

#[doc(inline)]
pub use crate::config::ReconnectOptions;
pub use crate::stream::{ReconnectStream, UnderlyingStream};

pub mod config;
mod stream;
