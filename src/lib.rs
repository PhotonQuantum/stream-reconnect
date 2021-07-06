//! Contains the ingredients needed to create wrappers over tokio AsyncRead/AsyncWrite items
//! to automatically reconnect upon failures. This is done so that a user can use them without worrying
//! that their application logic will terminate simply due to an event like a temporary network failure.
//!
//! This crate will try to provide commonly used io items, for example, the [StubbornTcpStream](StubbornTcpStream).
//! If you need to create your own, you simply need to implement the [UnderlyingIo](crate::tokio::UnderlyingIo) trait.
//! Once implemented, you can construct it easily by creating a [StubbornIo](crate::tokio::StubbornIo) type as seen below.
//!
//! *This crate requires at least version 1.39 of the Rust compiler.*
//!
//! ### Motivations
//! This crate was created because I was working on a service that needed to fetch data from a remote server
//! via a tokio TcpConnection. It normally worked perfectly (as does all of my code ☺), but every time the
//! remote server had a restart or turnaround, my application logic would stop working.
//! **stubborn-io** was born because I did not want to complicate my service's logic with TcpStream
//! reconnect and disconnect handling code. With stubborn-io, I can keep the service exactly the same,
//! knowing that the StubbornTcpStream's sensible defaults will perform reconnects in a way to keep my service running.
//! Once I realized that the implementation could apply to all IO items and not just TcpStream, I made it customizable as
//! seen below.
//!
//! ## Example on how a Stubborn IO item might be created
//! ```
//! # #[tokio::test]
//! # async fn test() -> std::io::Result<()> {
//! #    use crate::UnderlyingStream;
//! #    use std::future::Future;
//! #    use std::io;
//! #    use std::pin::Pin;
//! #    use tokio::net::TcpStream;
//! #    use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
//! #
//!     struct MyWs(WebSocketStream<MaybeTlsStream<TcpStream>>);
//!
//!     impl UnderlyingStream<String, io::Error> for MyWs {
//!         // Establishes an io connection.
//!         // Additionally, this will be used when reconnect tries are attempted.
//!         fn establish(addr: String) -> Pin<Box<dyn Future<Output = io::Result<Self>> + Send>> {
//!             Box::pin(async move {
//!                 // In this case, we are trying to connect to the WebSocket endpoint
//!                 let ws_connection = connect_async(addr).await.unwrap().0;
//!                 Ok(MyWs(ws_connection))
//!             })
//!         }
//!         fn exhaust_err() -> io::Error {
//!             io::Error::new(io::ErrorKind::Other, "Exhausted")
//!         }
//!         fn is_disconnect_error(&self, _err: &io::Error) -> bool {
//!             true
//!         }
//!     }
//! #     Ok(())
//! # }
//! ```

#[doc(inline)]
pub use crate::config::ReconnectOptions;
pub use crate::stream::{ReconnectStream, UnderlyingStream};

pub mod config;
mod stream;

#[cfg(all(feature = "tokio", feature = "async-std"))]
compile_error!("feature \"tokio\" and feature \"async-std\" cannot be enabled at the same time");
