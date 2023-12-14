mod config;
pub use config::*;

mod conn_state;
pub use conn_state::*;

mod listener;
pub use listener::*;

mod conn;
pub use conn::*;

mod stream;
pub use stream::*;

mod connector;
pub use connector::*;

mod acceptor;
pub use acceptor::*;

#[allow(unused)]
pub(crate) const MAX_DATAGRAM_SIZE: usize = 1350;

pub use quiche::{RecvInfo, SendInfo};

#[cfg(test)]
mod tests {

    use std::{io, net::SocketAddr, path::Path};

    use futures::task::SpawnExt;
    use quiche::RecvInfo;

    use super::*;

    fn config(is_server: bool) -> Config {
        let mut config = Config::new().unwrap();

        config.verify_peer(false);

        if is_server {
            let root_path = Path::new(env!("CARGO_MANIFEST_DIR"));

            log::debug!("test run dir {:?}", root_path);

            config
                .load_cert_chain_from_pem_file(root_path.join("cert/cert.crt").to_str().unwrap())
                .unwrap();

            config
                .load_priv_key_from_pem_file(root_path.join("cert/cert.key").to_str().unwrap())
                .unwrap();
        }

        config
            .set_application_protos(&[b"hq-interop", b"hq-29", b"hq-28", b"hq-27", b"http/0.9"])
            .unwrap();

        config.set_max_idle_timeout(5000);
        config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_initial_max_data(10_000_000);
        config.set_initial_max_stream_data_bidi_local(1_000_000);
        config.set_initial_max_stream_data_bidi_remote(1_000_000);
        config.set_initial_max_streams_bidi(100);
        config.set_initial_max_streams_uni(100);
        config.set_disable_active_migration(true);

        config
    }

    #[test]
    fn test_connect_accept() {
        let laddr = "127.0.0.1:10234".parse().unwrap();
        let raddr = "127.0.0.1:20234".parse().unwrap();

        let mut connector = Connector::new(&mut config(false), laddr, raddr).unwrap();

        let mut acceptor = QuicAcceptor::new(config(true)).unwrap();

        loop {
            let mut buf = [0; MAX_DATAGRAM_SIZE];

            let (send_size, send_info) = connector.send(&mut buf).unwrap();

            assert_eq!(send_info.from, laddr);
            assert_eq!(send_info.to, raddr);

            let (read_size, _) = acceptor
                .recv(
                    &mut buf[..send_size],
                    RecvInfo {
                        from: laddr,
                        to: raddr,
                    },
                )
                .unwrap();

            assert_eq!(read_size, send_size);

            let (send_size, send_info) = acceptor.send(&mut buf).unwrap();

            assert_eq!(send_info.from, raddr);
            assert_eq!(send_info.to, laddr);

            match acceptor.accept() {
                Ok(conns) => {
                    assert!(conns.len() == 1);
                    assert!(connector.is_established());
                    break;
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                r => {
                    r.unwrap();
                }
            }

            let read_size = connector
                .recv(
                    &mut buf[..send_size],
                    RecvInfo {
                        from: raddr,
                        to: laddr,
                    },
                )
                .unwrap();

            assert_eq!(read_size, send_size);
        }
    }

    #[hala_io_test::test]
    async fn test_async_api() {
        let raddr: SocketAddr = "127.0.0.1:20234".parse().unwrap();

        let mut conn = QuicConn::bind("127.0.0.1:0").unwrap();

        conn.connect(raddr, config(false), |fut| {
            hala_io_test::spawner().spawn(fut).unwrap();
        })
        .await
        .unwrap();
    }
}