//! Transport abstraction over Unix domain sockets and Windows named pipes.

use std::future::Future;
use std::io;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::endpoint::Endpoint;

/// Accepts incoming connections on a bound endpoint.
pub trait IpcListener: Send + 'static {
    /// Connection type produced by this listener.
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    /// Wait for the next client.
    fn accept(&mut self) -> impl Future<Output = io::Result<Self::Stream>> + Send;
}

/// A way of listening on and connecting to an [`Endpoint`].
pub trait IpcTransport {
    /// Connection type seen by the listening side.
    type ServerStream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    /// Connection type seen by the connecting side.
    type ClientStream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    /// Listener type returned by [`IpcTransport::bind`].
    type Listener: IpcListener<Stream = Self::ServerStream>;

    /// Start listening. Must be called inside a Tokio runtime.
    fn bind(endpoint: &Endpoint) -> io::Result<Self::Listener>;

    /// Connect to a listening endpoint.
    fn connect(endpoint: &Endpoint) -> impl Future<Output = io::Result<Self::ClientStream>> + Send;
}

#[cfg(unix)]
pub use self::unix::UnixTransport as NativeTransport;
#[cfg(windows)]
pub use self::windows::WindowsTransport as NativeTransport;

#[cfg(unix)]
mod unix {
    use std::io;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use tokio::net::{UnixListener, UnixStream};

    use super::{IpcListener, IpcTransport};
    use crate::endpoint::Endpoint;

    /// Unix domain socket transport.
    pub struct UnixTransport;

    /// Listening Unix socket; removes its socket file when dropped.
    pub struct UnixIpcListener {
        inner: UnixListener,
        path: PathBuf,
    }

    impl Drop for UnixIpcListener {
        fn drop(&mut self) {
            // Best effort: the runtime directory removal cleans up anyway.
            let _ = std::fs::remove_file(&self.path);
        }
    }

    impl IpcListener for UnixIpcListener {
        type Stream = UnixStream;

        async fn accept(&mut self) -> io::Result<UnixStream> {
            self.inner.accept().await.map(|(stream, _)| stream)
        }
    }

    impl IpcTransport for UnixTransport {
        type ServerStream = UnixStream;
        type ClientStream = UnixStream;
        type Listener = UnixIpcListener;

        fn bind(endpoint: &Endpoint) -> io::Result<UnixIpcListener> {
            let path = PathBuf::from(endpoint.as_str());
            // A leftover socket file from a crashed run would make bind fail.
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            let inner = UnixListener::bind(&path)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            Ok(UnixIpcListener { inner, path })
        }

        async fn connect(endpoint: &Endpoint) -> io::Result<UnixStream> {
            UnixStream::connect(endpoint.as_str()).await
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::time::Duration;

    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    };

    use super::{IpcListener, IpcTransport};
    use crate::endpoint::Endpoint;

    /// `ERROR_PIPE_BUSY`: all pipe instances are momentarily taken.
    const ERROR_PIPE_BUSY: i32 = 231;

    /// Named pipe transport.
    pub struct WindowsTransport;

    /// A named pipe waiting for its next client.
    pub struct PipeListener {
        name: String,
        pending: NamedPipeServer,
    }

    impl IpcListener for PipeListener {
        type Stream = NamedPipeServer;

        async fn accept(&mut self) -> io::Result<NamedPipeServer> {
            self.pending.connect().await?;
            // Open the next instance before handing this one out, so clients
            // arriving meanwhile never see a missing pipe.
            let next = ServerOptions::new().create(&self.name)?;
            Ok(std::mem::replace(&mut self.pending, next))
        }
    }

    impl IpcTransport for WindowsTransport {
        type ServerStream = NamedPipeServer;
        type ClientStream = NamedPipeClient;
        type Listener = PipeListener;

        fn bind(endpoint: &Endpoint) -> io::Result<PipeListener> {
            let name = endpoint.as_str().to_string();
            let pending = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&name)?;
            Ok(PipeListener { name, pending })
        }

        async fn connect(endpoint: &Endpoint) -> io::Result<NamedPipeClient> {
            // Retry briefly while every instance is busy serving someone else.
            loop {
                match ClientOptions::new().open(endpoint.as_str()) {
                    Ok(client) => return Ok(client),
                    Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }
}
