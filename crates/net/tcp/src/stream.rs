use std::{
    fmt::Debug,
    io,
    net::{Shutdown, SocketAddr, ToSocketAddrs},
    task::Poll,
};

#[cfg(feature = "current")]
use hala_io::current::*;
use hala_io::*;

use futures::{AsyncRead, AsyncWrite};

/// A TCP stream between a local and a remote socket.
pub struct TcpStream {
    pub fd: Handle,
    poller: Handle,
    driver: Driver,
}

impl Debug for TcpStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TcpStream({:?})", self.fd)
    }
}

impl TcpStream {
    pub(super) fn new_with(driver: Driver, fd: Handle, poller: Handle) -> io::Result<Self> {
        match driver.fd_cntl(
            poller,
            Cmd::Register {
                source: fd,
                interests: Interest::Readable | Interest::Writable,
            },
        ) {
            Err(err) => {
                _ = driver.fd_close(fd);
                return Err(err);
            }
            _ => {}
        }

        Ok(Self { fd, driver, poller })
    }

    /// Opens a TCP connection to a remote host with global context `poller`
    pub fn connect<S: ToSocketAddrs>(raddrs: S) -> io::Result<Self> {
        Self::connect_with(raddrs, get_poller()?)
    }

    /// Opens a TCP connection to a remote host with customer `poller` handle.
    pub fn connect_with<S: ToSocketAddrs>(raddrs: S, poller: Handle) -> io::Result<Self> {
        let driver = get_driver()?;

        let raddrs = raddrs.to_socket_addrs()?.into_iter().collect::<Vec<_>>();

        let fd = driver.fd_open(Description::TcpStream, OpenFlags::Connect(&raddrs))?;

        Self::new_with(driver, fd, poller)
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.driver
            .fd_cntl(self.fd, Cmd::LocalAddr)?
            .try_into_sockaddr()
    }

    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.driver.fd_cntl(self.fd, Cmd::Shutdown(how))?;

        Ok(())
    }
}

impl AsyncWrite for &TcpStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        poll_would_block(|| {
            self.driver
                .fd_cntl(
                    self.fd,
                    Cmd::Write {
                        waker: cx.waker().clone(),
                        buf,
                    },
                )?
                .try_into_datalen()
        })
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for &TcpStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        poll_would_block(|| {
            self.driver
                .fd_cntl(
                    self.fd,
                    Cmd::Read {
                        waker: cx.waker().clone(),
                        buf,
                    },
                )?
                .try_into_datalen()
        })
    }
}

impl AsyncWrite for TcpStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        poll_would_block(|| {
            self.driver
                .fd_cntl(
                    self.fd,
                    Cmd::Write {
                        waker: cx.waker().clone(),
                        buf,
                    },
                )?
                .try_into_datalen()
        })
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for TcpStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        poll_would_block(|| {
            self.driver
                .fd_cntl(
                    self.fd,
                    Cmd::Read {
                        waker: cx.waker().clone(),
                        buf,
                    },
                )?
                .try_into_datalen()
        })
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        self.driver
            .fd_cntl(self.poller, Cmd::Deregister(self.fd))
            .unwrap();
        self.driver.fd_close(self.fd).unwrap()
    }
}
