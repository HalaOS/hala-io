use std::{
    collections::{HashSet, VecDeque},
    io,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::Poll,
};

use future_mediator::{LocalMediator, SharedData};
use futures::FutureExt;
use hala_io_util::{get_local_poller, local_io_spawn, Sleep};
use quiche::{RecvInfo, SendInfo};

use crate::{errors::into_io_error, quic::QuicStream};

/// Quic connection state object
pub struct QuicConnState {
    /// quiche connection instance.
    quiche_conn: quiche::Connection,
    /// Opened stream id set
    opened_streams: HashSet<u64>,
    /// Incoming stream deque.
    incoming_streams: VecDeque<u64>,
}

impl QuicConnState {
    /// Create new `QuicConnState` from [`Connection`](quiche::Connection)
    pub fn new(quiche_conn: quiche::Connection) -> Self {
        Self {
            quiche_conn,
            opened_streams: Default::default(),
            incoming_streams: Default::default(),
        }
    }
}

impl Drop for QuicConnState {
    fn drop(&mut self) {
        log::trace!("dropping conn={}", self.quiche_conn.trace_id());
    }
}

/// `QuicConnState` support event variant.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum QuicConnEvents {
    Send(String),
    Recv(String),
    StreamSend(String, u64),
    StreamRecv(String, u64),
    Accept(String),
    OpenStream,
}

fn handle_accept(cx: &mut SharedData<QuicConnState, QuicConnEvents>, stream_id: u64) {
    if !cx.opened_streams.contains(&stream_id) {
        log::trace!(
            "handle incoming, conn={:?}, stream={}",
            cx.quiche_conn.trace_id(),
            stream_id
        );

        cx.opened_streams.insert(stream_id);
        cx.incoming_streams.push_back(stream_id);

        cx.notify(QuicConnEvents::Accept(cx.quiche_conn.trace_id().into()));
    }
}

fn handle_stream(cx: &mut SharedData<QuicConnState, QuicConnEvents>) {
    for stream_id in cx.quiche_conn.readable() {
        handle_accept(cx, stream_id);
        cx.notify(QuicConnEvents::StreamRecv(
            cx.quiche_conn.trace_id().into(),
            stream_id,
        ));
    }

    for stream_id in cx.quiche_conn.writable() {
        handle_accept(cx, stream_id);

        cx.notify(QuicConnEvents::StreamSend(
            cx.quiche_conn.trace_id().into(),
            stream_id,
        ));
    }
}

fn handle_close(cx: &mut SharedData<QuicConnState, QuicConnEvents>) {
    cx.wakeup_all();
}

/// Quic connection state object
#[derive(Clone)]
pub struct AsyncQuicConnState {
    /// core inner state.
    pub(crate) state: LocalMediator<QuicConnState, QuicConnEvents>,
    /// stream id generator seed
    stream_id_seed: Arc<AtomicU64>,
    /// String type trace id.
    pub trace_id: Arc<String>,
}

impl AsyncQuicConnState {
    pub fn new(quiche_conn: quiche::Connection, stream_id_seed: u64) -> Self {
        Self {
            trace_id: Arc::new(quiche_conn.trace_id().to_owned()),
            state: LocalMediator::new_with(
                QuicConnState::new(quiche_conn),
                "mediator: quic_conn_state",
            ),
            stream_id_seed: Arc::new(stream_id_seed.into()),
        }
    }

    /// Create new future for send connection data
    pub async fn send<'a>(&self, buf: &'a mut [u8]) -> io::Result<(usize, SendInfo)> {
        let mut sleep: Option<Sleep> = None;

        let event = QuicConnEvents::Send(self.trace_id.to_string());

        self.state
            .on_poll(event.clone(), |state, cx| {
                if state.quiche_conn.is_closed() {
                    handle_close(state);

                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        format!("{:?} err=broken_pipe", event,),
                    )));
                }

                if let Some(mut sleep) = sleep.take() {
                    match sleep.poll_unpin(cx) {
                        Poll::Ready(_) => {
                            log::trace!("{:?} on_timeout", event);
                            state.quiche_conn.on_timeout();
                        }
                        Poll::Pending => {}
                    }
                }

                loop {
                    match state.quiche_conn.send(buf) {
                        Ok((send_size, send_info)) => {
                            log::trace!(
                                "{:?}, send_size={}, send_info={:?}",
                                event,
                                send_size,
                                send_info
                            );

                            handle_stream(state);

                            return Poll::Ready(Ok((send_size, send_info)));
                        }
                        Err(quiche::Error::Done) => {
                            if state.quiche_conn.is_closed() {
                                handle_close(state);
                                return Poll::Ready(Err(io::Error::new(
                                    io::ErrorKind::BrokenPipe,
                                    format!("{:?} err=broken_pipe", event,),
                                )));
                            }

                            if let Some(expired) = state.quiche_conn.timeout() {
                                log::trace!("{:?} add timeout({:?})", event, expired);

                                if expired.is_zero() {
                                    state.quiche_conn.on_timeout();
                                    continue;
                                }

                                let mut timeout = Sleep::new_with(get_local_poller()?, expired)?;

                                match timeout.poll_unpin(cx) {
                                    Poll::Ready(_) => {
                                        log::trace!("{:?} on_timeout immediately", event);

                                        state.quiche_conn.on_timeout();
                                        continue;
                                    }
                                    _ => {
                                        sleep = Some(timeout);
                                    }
                                }
                            }

                            return Poll::Pending;
                        }
                        Err(err) => return Poll::Ready(Err(into_io_error(err))),
                    }
                }
            })
            .await
    }

    /// Create new future for recv connection data
    pub async fn recv<'a>(&self, buf: &'a mut [u8], recv_info: RecvInfo) -> io::Result<usize> {
        self.state
            .on_poll(
                QuicConnEvents::Recv(self.trace_id.to_string()),
                |state, _| {
                    if state.quiche_conn.is_closed() {
                        handle_close(state);
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            format!("conn={} closed", state.quiche_conn.trace_id()),
                        )));
                    }

                    let recv_info = recv_info;

                    match state.quiche_conn.recv(buf, recv_info) {
                        Ok(recv_size) => {
                            log::trace!(
                                "conn={} recv data, len={}",
                                state.quiche_conn.trace_id(),
                                recv_size
                            );

                            if state.quiche_conn.is_closed() {
                                handle_close(state);
                            } else {
                                // wakeup send poll
                                state.notify(QuicConnEvents::Send(self.trace_id.to_string()));

                                // wakeup stream
                                handle_stream(state);
                            }

                            return Poll::Ready(Ok(recv_size));
                        }
                        Err(quiche::Error::Done) => {
                            if state.quiche_conn.is_closed() {
                                handle_close(state);
                            }
                            return Poll::Pending;
                        }
                        Err(err) => {
                            if state.quiche_conn.is_closed() {
                                handle_close(state);
                            }
                            return Poll::Ready(Err(into_io_error(err)));
                        }
                    }
                },
            )
            .await
    }

    /// Create new future for send stream data
    pub async fn stream_send<'a>(
        &self,
        stream_id: u64,
        buf: &'a [u8],
        fin: bool,
    ) -> io::Result<usize> {
        self.state
            .on_poll(
                QuicConnEvents::StreamSend(self.trace_id.to_string(), stream_id),
                |state, _| {
                    if state.quiche_conn.is_closed() {
                        handle_close(state);

                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            format!("conn={} closed", state.quiche_conn.trace_id()),
                        )));
                    }

                    match state.quiche_conn.stream_send(stream_id, buf, fin) {
                        Ok(recv_size) => {
                            state.notify(QuicConnEvents::Send(self.trace_id.to_string()));

                            if fin {
                                state.opened_streams.remove(&stream_id);
                            }

                            return Poll::Ready(Ok(recv_size));
                        }
                        Err(quiche::Error::Done) => {
                            log::trace!(
                                "StreamSend({}, {}) done ",
                                self.trace_id.to_string(),
                                stream_id
                            );

                            if state.quiche_conn.is_closed() {
                                handle_close(state);

                                return Poll::Ready(Err(io::Error::new(
                                    io::ErrorKind::BrokenPipe,
                                    format!("conn={} closed", state.quiche_conn.trace_id()),
                                )));
                            }

                            return Poll::Pending;
                        }
                        Err(err) => {
                            if fin {
                                state.opened_streams.remove(&stream_id);
                            }

                            if state.quiche_conn.is_closed() {
                                handle_close(state);
                            }

                            Poll::Ready(Err(into_io_error(err)))
                        }
                    }
                },
            )
            .await
    }

    /// Create new future for recv stream data
    pub async fn stream_recv<'a>(
        &self,
        stream_id: u64,
        buf: &'a mut [u8],
    ) -> io::Result<(usize, bool)> {
        self.state
            .on_poll(
                QuicConnEvents::StreamRecv(self.trace_id.to_string(), stream_id),
                |state, _| {
                    if state.quiche_conn.is_closed() {
                        handle_close(state);
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            format!("conn={} closed", state.quiche_conn.trace_id()),
                        )));
                    }

                    match state.quiche_conn.stream_recv(stream_id, buf) {
                        Ok(recv_size) => {
                            if state.quiche_conn.is_closed() {
                                handle_close(state);
                            } else {
                                state.notify(QuicConnEvents::Recv(self.trace_id.to_string()));
                                state.notify(QuicConnEvents::Send(self.trace_id.to_string()));
                            }

                            return Poll::Ready(Ok(recv_size));
                        }
                        Err(quiche::Error::Done) => {
                            if state.quiche_conn.is_closed() {
                                handle_close(state);

                                return Poll::Ready(Err(io::Error::new(
                                    io::ErrorKind::BrokenPipe,
                                    format!("conn={} closed", state.quiche_conn.trace_id()),
                                )));
                            }

                            log::trace!(
                                "{:?} Pending",
                                QuicConnEvents::StreamRecv(self.trace_id.to_string(), stream_id)
                            );

                            return Poll::Pending;
                        }
                        Err(err) => {
                            if state.quiche_conn.is_closed() {
                                handle_close(state);
                            }
                            return Poll::Ready(Err(into_io_error(err)));
                        }
                    }
                },
            )
            .await
    }

    /// Open new stream to communicate with remote peer.
    pub async fn open_stream(&self) -> io::Result<QuicStream> {
        let id = self.stream_id_seed.fetch_add(4, Ordering::SeqCst);

        self.state
            .on_poll(QuicConnEvents::OpenStream, |state, _| {
                if state.quiche_conn.is_closed() {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        format!("Quic conn closed: {}", state.quiche_conn.trace_id()),
                    )));
                }

                log::trace!(
                    "create new stream, stream_id={}, conn_id={}",
                    id,
                    state.quiche_conn.trace_id()
                );

                state.notify(QuicConnEvents::Send(self.trace_id.to_string()));

                Poll::Ready(Ok(QuicStream::new(id, self.clone())))
            })
            .await
    }

    pub fn close_stream(&self, stream_id: u64) {
        let this = self.clone();

        local_io_spawn(async move {
            this.stream_send(stream_id, b"", true).await?;

            Ok(())
        })
        .unwrap();
    }

    pub(super) async fn is_stream_closed(&self, stream_id: u64) -> bool {
        self.state
            .with(|mediator| mediator.quiche_conn.stream_finished(stream_id))
    }

    /// Close connection.
    pub(super) async fn close(&self, app: bool, err: u64, reason: &[u8]) -> io::Result<()> {
        self.state.with_mut(|state| {
            state
                .quiche_conn
                .close(app, err, reason)
                .map_err(into_io_error)
        })
    }

    pub(super) async fn is_closed(&self) -> bool {
        self.state.with(|state| state.quiche_conn.is_closed())
    }

    pub async fn accept(&self) -> Option<QuicStream> {
        let event = QuicConnEvents::Accept(self.trace_id.to_string());

        self.state
            .on_poll(event.clone(), |state, _| {
                log::trace!("{:?} poll once", event);

                if state.quiche_conn.is_closed() {
                    log::trace!("{:?}, conn_status=closed", event);
                    return Poll::Ready(None);
                }

                if let Some(stream_id) = state.incoming_streams.pop_front() {
                    return Poll::Ready(Some(QuicStream::new(stream_id, self.clone())));
                } else {
                    return Poll::Pending;
                }
            })
            .await
    }
}
