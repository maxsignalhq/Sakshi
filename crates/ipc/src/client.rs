//! The sending side, used by hooks. Built to fail open: callers get an error
//! to ignore, never a hang.

use std::io;
use std::time::Duration;

use sakshi_core::Event;
use tokio::io::AsyncWriteExt;

use crate::endpoint::Endpoint;
use crate::protocol::encode_line;
use crate::transport::{IpcTransport, NativeTransport};

/// Send `events` to the server at `endpoint` over the native transport.
pub async fn send_events(
    endpoint: &Endpoint,
    events: &[Event],
    timeout: Duration,
) -> io::Result<()> {
    send_events_with::<NativeTransport>(endpoint, events, timeout).await
}

/// Send `events` over a specific transport. Fails with
/// [`io::ErrorKind::TimedOut`] if connecting and writing takes longer than `timeout`.
pub async fn send_events_with<T: IpcTransport>(
    endpoint: &Endpoint,
    events: &[Event],
    timeout: Duration,
) -> io::Result<()> {
    let mut payload = String::new();
    for event in events {
        payload.push_str(&encode_line(event));
        payload.push('\n');
    }
    let send = async {
        let mut stream = T::connect(endpoint).await?;
        stream.write_all(payload.as_bytes()).await?;
        stream.flush().await?;
        stream.shutdown().await
    };
    match tokio::time::timeout(timeout, send).await {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "timed out talking to sakshi",
        )),
    }
}
