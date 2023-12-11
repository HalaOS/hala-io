use std::{io, net::SocketAddr};

use quiche::{RecvInfo, SendInfo};
use ring::rand::{SecureRandom, SystemRandom};

use crate::errors::into_io_error;

use super::{inner_conn::QuicInnerConn, Config};

/// Quic client connector
pub struct Connector {
    /// source connection id.
    pub(super) quiche_conn: quiche::Connection,
}

impl Connector {
    /// Create new quic connector
    pub fn new(mut config: Config, laddr: SocketAddr, raddr: SocketAddr) -> io::Result<Connector> {
        let mut scid = vec![0; quiche::MAX_CONN_ID_LEN];

        SystemRandom::new().fill(&mut scid).map_err(into_io_error)?;

        let scid = quiche::ConnectionId::from_vec(scid);

        log::trace!("Connector {:?}", scid);

        let quiche_conn = quiche::connect(None, &scid, laddr, raddr, &mut config)
            .map_err(|err| io::Error::new(io::ErrorKind::ConnectionRefused, err))?;

        Ok(Self { quiche_conn })
    }

    /// Generate send data.
    pub fn send(&mut self, buf: &mut [u8]) -> io::Result<(usize, SendInfo)> {
        self.quiche_conn
            .send(buf)
            .map_err(|err| io::Error::new(io::ErrorKind::ConnectionRefused, err))
    }

    /// Accept remote peer data.
    pub fn recv(&mut self, buf: &mut [u8], recv_info: RecvInfo) -> io::Result<usize> {
        let len = self
            .quiche_conn
            .recv(buf, recv_info)
            .map_err(|err| io::Error::new(io::ErrorKind::ConnectionRefused, err))?;

        if self.quiche_conn.is_closed() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                "Early stage reject",
            ));
        }

        Ok(len)
    }

    /// Check if underly connection is established.
    pub fn is_established(&self) -> bool {
        self.quiche_conn.is_established()
    }
}

impl From<Connector> for QuicInnerConn {
    fn from(value: Connector) -> Self {
        QuicInnerConn::new(value.quiche_conn)
    }
}
