//! Seekable async stream over a single file inside a torrent.

use std::io::SeekFrom;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncSeek, ReadBuf};

/// Trait alias for the capabilities of a torrent file stream.
///
/// `librqbit`'s concrete `FileStream` type is not nameable outside its crate
/// (it lives in a private module), so the engine boxes it behind this trait.
pub trait AsyncFileStream: AsyncRead + AsyncSeek + Send + Unpin + 'static {}

impl<T> AsyncFileStream for T where T: AsyncRead + AsyncSeek + Send + Unpin + 'static {}

/// A seekable async reader over one file of a torrent.
///
/// Reads block (asynchronously) until the pieces covering the read position
/// have been downloaded and verified. Implements [`AsyncRead`] and
/// [`AsyncSeek`], so the usual `tokio::io` extension traits apply.
pub struct FileStream {
    inner: Pin<Box<dyn AsyncFileStream>>,
}

impl FileStream {
    pub(crate) fn new(stream: impl AsyncFileStream) -> Self {
        Self {
            inner: Box::pin(stream),
        }
    }
}

impl std::fmt::Debug for FileStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileStream").finish_non_exhaustive()
    }
}

impl AsyncRead for FileStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.inner.as_mut().poll_read(cx, buf)
    }
}

impl AsyncSeek for FileStream {
    fn start_seek(mut self: Pin<&mut Self>, position: SeekFrom) -> std::io::Result<()> {
        self.inner.as_mut().start_seek(position)
    }

    fn poll_complete(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<u64>> {
        self.inner.as_mut().poll_complete(cx)
    }
}
