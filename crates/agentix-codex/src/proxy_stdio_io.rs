//! Cancellable standard I/O for Unix pipes/terminals, without Tokio blocking workers.
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::fs::FileTypeExt;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, Interest, ReadBuf, unix::AsyncFd};

struct Descriptor {
    file: File,
    flags: OFlag,
}
impl AsRawFd for Descriptor {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.file.as_raw_fd()
    }
}
impl Drop for Descriptor {
    fn drop(&mut self) {
        let _ = fcntl(self.file.as_fd(), FcntlArg::F_SETFL(self.flags));
    }
}
enum Handle {
    Ready(AsyncFd<Descriptor>),
    File(Descriptor),
}
pub(super) struct Stdio(Handle);
impl Stdio {
    pub(super) fn pair(input: BorrowedFd<'_>, output: BorrowedFd<'_>) -> io::Result<(Self, Self)> {
        // Capture both flags before changing either: stdin/stdout may be dup'd
        // from one open file description (for example a terminal or socket).
        let output_flags = OFlag::from_bits_truncate(fcntl(output, FcntlArg::F_GETFL)?);
        let input = Self::new(input, Interest::READABLE)?;
        let mut output = Self::new(output, Interest::WRITABLE)?;
        match &mut output.0 {
            Handle::Ready(fd) => fd.get_mut().flags = output_flags,
            Handle::File(fd) => fd.flags = output_flags,
        }
        Ok((input, output))
    }

    pub(super) fn new(fd: BorrowedFd<'_>, interest: Interest) -> io::Result<Self> {
        let file = File::from(fd.try_clone_to_owned()?);
        let flags = OFlag::from_bits_truncate(fcntl(&file, FcntlArg::F_GETFL)?);
        let descriptor = Descriptor { file, flags };
        fcntl(
            &descriptor.file,
            FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK),
        )?;
        // Regular files and /dev/null do not support epoll/kqueue readiness.
        let kind = descriptor.file.metadata()?.file_type();
        if kind.is_file() || (kind.is_char_device() && !nix::unistd::isatty(&descriptor.file)?) {
            return Ok(Self(Handle::File(descriptor)));
        }
        Ok(Self(Handle::Ready(AsyncFd::with_interest(
            descriptor, interest,
        )?)))
    }
}
impl AsyncRead for Stdio {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let count = match &self.0 {
            Handle::File(fd) => (&fd.file).read(buf.initialize_unfilled())?,
            Handle::Ready(fd) => loop {
                let mut ready = ready!(fd.poll_read_ready(cx))?;
                if let Ok(result) =
                    ready.try_io(|fd| (&fd.get_ref().file).read(buf.initialize_unfilled()))
                {
                    break result?;
                }
            },
        };
        buf.advance(count);
        Poll::Ready(Ok(()))
    }
}
impl AsyncWrite for Stdio {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &self.0 {
            Handle::File(fd) => Poll::Ready((&fd.file).write(bytes)),
            Handle::Ready(fd) => loop {
                let mut ready = ready!(fd.poll_write_ready(cx))?;
                if let Ok(result) = ready.try_io(|fd| (&fd.get_ref().file).write(bytes)) {
                    return Poll::Ready(result);
                }
            },
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(())) // Writes go directly to the descriptor, with no user-space buffer.
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn redirected_regular_files_preserve_bytes_and_descriptor_flags() {
        use std::io::{Seek, SeekFrom};
        let file = tempfile::tempfile().unwrap();
        // Write first: macOS adds a kernel-maintained flag after the first write.
        (&file).write_all(b"seed").unwrap();
        (&file).seek(SeekFrom::Start(0)).unwrap();
        let flags = fcntl(&file, FcntlArg::F_GETFL).unwrap();
        let mut output = Stdio::new(file.as_fd(), Interest::WRITABLE).unwrap();
        output.write_all(b"exact\n").await.unwrap();
        drop(output);
        assert_eq!(fcntl(&file, FcntlArg::F_GETFL).unwrap(), flags);
        (&file).seek(SeekFrom::Start(0)).unwrap();
        let mut input = Stdio::new(file.as_fd(), Interest::READABLE).unwrap();
        let mut bytes = Vec::new();
        input.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"exact\n");
    }
    #[tokio::test]
    async fn shared_input_output_description_restores_original_flags() {
        let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let flags = fcntl(&socket, FcntlArg::F_GETFL).unwrap();
        let (input, output) = Stdio::pair(socket.as_fd(), socket.as_fd()).unwrap();
        drop(input);
        drop(output);
        assert_eq!(fcntl(&socket, FcntlArg::F_GETFL).unwrap(), flags);
    }
    #[tokio::test]
    async fn shared_stdio_socket_reads_writes_and_restores_flags() {
        use std::time::Duration;
        let (socket, mut peer) = std::os::unix::net::UnixStream::pair().unwrap();
        // Stabilize macOS's kernel-maintained first-write flag before comparing.
        File::from(socket.as_fd().try_clone_to_owned().unwrap())
            .write_all(b"x")
            .unwrap();
        peer.read_exact(&mut [0; 1]).unwrap();
        let flags = fcntl(&socket, FcntlArg::F_GETFL).unwrap();
        let (mut input, mut output) = Stdio::pair(socket.as_fd(), socket.as_fd()).unwrap();
        peer.write_all(b"request\n").unwrap();
        let mut bytes = [0; 8];
        tokio::time::timeout(Duration::from_secs(1), input.read_exact(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&bytes, b"request\n");
        tokio::time::timeout(Duration::from_secs(1), output.write_all(b"reply\n"))
            .await
            .unwrap()
            .unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let mut reply = [0; 6];
        peer.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"reply\n");
        drop(output);
        drop(input);
        assert_eq!(fcntl(&socket, FcntlArg::F_GETFL).unwrap(), flags);
    }

    #[tokio::test]
    async fn initially_nonblocking_descriptors_remain_nonblocking() {
        let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        socket.set_nonblocking(true).unwrap();
        let flags = fcntl(&socket, FcntlArg::F_GETFL).unwrap();
        let (input, output) = Stdio::pair(socket.as_fd(), socket.as_fd()).unwrap();
        drop(input);
        drop(output);
        assert_eq!(fcntl(&socket, FcntlArg::F_GETFL).unwrap(), flags);
    }
}
