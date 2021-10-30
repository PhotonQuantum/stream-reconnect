# stream-reconnect

[![crates.io](https://img.shields.io/crates/v/stream-reconnect?style=flat-square)](https://crates.io/crates/stream-reconnect)
[![Documentation](https://img.shields.io/docsrs/stream-reconnect?style=flat-square)](https://docs.rs/stream-reconnect)

This crate provides a `Stream`/`Sink`-wrapping struct that automatically recover from potential
disconnections/interruptions.

This is a fork of [stubborn-io](https://github.com/craftytrickster/stubborn-io), which is built for the same purpose but
for `AsyncRead`/`AsyncWrite`.

To use with your project, add the following to your Cargo.toml:

```toml
stream-reconnect = "0.3"
```

*Minimum supported rust version: 1.43.1*

## Runtime Support

This crate supports both `tokio` and `async-std` runtime.

`tokio` support is enabled by default. While used on an `async-std` runtime, change the corresponding dependency
in `Cargo.toml` to

```toml
stream-reconnect = { version = "0.3", default-features = false, features = ["async-std"] }
```

## Feature Gates

`not-send` - allow the establish function to be non thread-safe.

## Example

In this example, we will see a drop in replacement for tungstenite's WebSocketStream, with the distinction that it will
automatically attempt to reconnect in the face of connectivity failures.

```rust
struct MyWs;

impl UnderlyingStream<String, Result<Message, WsError>, WsError> for MyWs {
    type Stream = WebSocketStream<MaybeTlsStream<TcpStream>>;

    // Establishes connection.
    // Additionally, this will be used when reconnect tries are attempted.
    fn establish(addr: String) -> Pin<Box<dyn Future<Output=Result<Self::Stream, WsError>> + Send>> {
        Box::pin(async move {
            // In this case, we are trying to connect to the WebSocket endpoint
            let ws_connection = connect_async(addr).await.unwrap().0;
            Ok(ws_connection)
        })
    }

    // The following errors are considered disconnect errors.
    fn is_write_disconnect_error(err: &WsError) -> bool {
        matches!(
                err,
                WsError::ConnectionClosed
                    | WsError::AlreadyClosed
                    | WsError::Io(_)
                    | WsError::Tls(_)
                    | WsError::Protocol(_)
            )
    }

    // If an `Err` is read, then there might be an disconnection.
    fn is_read_disconnect_error(item: &Result<Message, WsError>) -> bool {
        if let Err(e) = item {
            Self::is_write_disconnect_error(e)
        } else {
            false
        }
    }

    // Return "Exhausted" if all retry attempts are failed.
    fn exhaust_err() -> WsError {
        WsError::Io(io::Error::new(io::ErrorKind::Other, "Exhausted"))
    }
}

type ReconnectWs = ReconnectStream<MyWs, String, Result<Message, WsError>, WsError>;

#[tokio::main]
async fn main() {
    let mut ws_stream: ReconnectWs = ReconnectWs::connect(String::from("wss://localhost:8000"));
    ws_stream.send("hello world!").await.unwrap();
}
```

## License

MIT