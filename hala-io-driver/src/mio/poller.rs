use std::{
    collections::VecDeque,
    fmt::Debug,
    io,
    sync::Arc,
    task::Poll,
    time::{Duration, Instant},
};

use timewheel::TimeWheel;

use crate::{Handle, Interest, Token, TypedHandle};

use super::*;

fn duration_to_ticks(duration: Duration, tick_duration: Duration, round_up: bool) -> u128 {
    let duration_m = duration.as_nanos();
    let tick_duration_m = tick_duration.as_nanos();

    let mut ticks = duration_m / tick_duration_m;

    if round_up && tick_duration * (ticks as u32) < duration {
        ticks += 1;
    }

    ticks
}

pub(crate) struct MioTimeout {
    pub(crate) duration: Duration,
    pub(crate) start_time: Option<Instant>,
    pub(crate) slot: Option<u128>,
    pub(crate) tick_duration: Option<Duration>,
}

impl Debug for MioTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(start_time) = self.start_time.as_ref() {
            write!(
                f,
                "timeout={:?},elapsed {:?}",
                self.duration,
                start_time.elapsed()
            )
        } else {
            write!(f, "timeout={:?}", self.duration)
        }
    }
}

impl MioTimeout {
    pub(crate) fn new(duration: Duration) -> Self {
        Self {
            duration,
            start_time: None,
            slot: None,
            tick_duration: None,
        }
    }

    pub(crate) fn is_register(&self) -> bool {
        self.start_time.is_some() && self.slot.is_some()
    }

    pub(crate) fn is_expired(&self) -> bool {
        if let Some(start_time) = self.start_time {
            let elapsed = start_time.elapsed();

            if elapsed >= self.duration {
                return true;
            }

            if self.duration - elapsed < self.tick_duration.unwrap() {
                return true;
            }

            return false;
        } else {
            false
        }
    }
}

pub trait MioPoller {
    fn poll_once(&self, timeout: Option<Duration>) -> io::Result<Vec<(Token, Interest)>>;
    fn register(&self, handle: Handle, interests: Interest) -> io::Result<()>;
    fn reregister(&self, handle: Handle, interests: Interest) -> io::Result<()>;
    fn deregister(&self, handle: Handle) -> io::Result<()>;
}

struct MioTimeWheel {
    time_wheel: TimeWheel<Token>,
    last_tick: Instant,
}

impl MioTimeWheel {
    fn new() -> Self {
        Self {
            time_wheel: TimeWheel::new(2048),
            last_tick: Instant::now(),
        }
    }

    fn tick(&mut self, tick_duration: Duration) -> VecDeque<Token> {
        let elapsed = self.last_tick.elapsed();

        let ticks = duration_to_ticks(elapsed, tick_duration, false);

        let ticks = ticks - self.time_wheel.tick;

        let mut timeout_vec = VecDeque::new();

        for _ in 0..ticks {
            if let Poll::Ready(mut v) = self.time_wheel.tick() {
                for token in &v {
                    log::trace!("{:?} timeout expired, at={}", token, self.time_wheel.tick)
                }

                timeout_vec.append(&mut v);
            }
        }

        timeout_vec
    }
}

pub struct BasicMioPoller<TM: ThreadModel> {
    poll: TM::Guard<mio::Poll>,
    registry: Arc<mio::Registry>,
    time_wheel: TM::Guard<MioTimeWheel>,
    tick_duration: Duration,
}

impl<TM> Clone for BasicMioPoller<TM>
where
    TM: ThreadModel,
{
    fn clone(&self) -> Self {
        Self {
            poll: self.poll.clone(),
            registry: self.registry.clone(),
            time_wheel: self.time_wheel.clone(),
            tick_duration: self.tick_duration.clone(),
        }
    }
}

impl<TM> Default for BasicMioPoller<TM>
where
    TM: ThreadModel,
{
    fn default() -> Self {
        let poll = mio::Poll::new().unwrap();

        let registry = poll.registry().try_clone().unwrap();

        Self {
            poll: poll.into(),
            registry: registry.into(),
            time_wheel: MioTimeWheel::new().into(),
            tick_duration: Duration::from_millis(1),
        }
    }
}

impl<TM> MioPoller for BasicMioPoller<TM>
where
    TM: ThreadModel,
{
    fn poll_once(&self, timeout: Option<Duration>) -> io::Result<Vec<(Token, Interest)>> {
        let poll_timeout = timeout.unwrap_or(self.tick_duration);

        let mut events = mio::event::Events::with_capacity(1024);

        self.poll.get_mut().poll(&mut events, Some(poll_timeout))?;

        let mut hala_events = vec![];

        for event in events.iter() {
            let mut interests = Interest::Readable | Interest::Writable;

            if !event.is_readable() {
                interests = interests ^ Interest::Readable;
            } else if !event.is_writable() {
                interests = interests ^ Interest::Writable;
            }

            hala_events.push((Token(event.token().0), interests));
        }

        let timeout = self.time_wheel.get_mut().tick(self.tick_duration);

        for token in timeout {
            hala_events.push((token, Interest::Readable));
        }

        Ok(hala_events)
    }

    fn register(&self, handle: Handle, interests: Interest) -> io::Result<()> {
        let mut mio_interests = mio::Interest::READABLE.add(mio::Interest::WRITABLE);

        if !interests.contains(Interest::Writable) {
            mio_interests = mio_interests.remove(mio::Interest::WRITABLE).unwrap();
        }

        if !interests.contains(Interest::Readable) {
            mio_interests = mio_interests.remove(mio::Interest::READABLE).unwrap();
        }

        match handle.desc {
            crate::Description::File => todo!(),
            crate::Description::TcpListener => TypedHandle::<mio::net::TcpListener>::new(handle)
                .with_mut(|source| {
                    self.registry
                        .register(source, mio::Token(handle.token.0), mio_interests)
                }),
            crate::Description::TcpStream => TypedHandle::<mio::net::TcpStream>::new(handle)
                .with_mut(|source| {
                    self.registry
                        .register(source, mio::Token(handle.token.0), mio_interests)
                }),
            crate::Description::UdpSocket => TypedHandle::<mio::net::UdpSocket>::new(handle)
                .with_mut(|source| {
                    self.registry
                        .register(source, mio::Token(handle.token.0), mio_interests)
                }),
            crate::Description::Timeout => {
                TypedHandle::<MioTimeout>::new(handle).with_mut(|timeout| {
                    assert!(!timeout.duration.is_zero());

                    let ticks = duration_to_ticks(timeout.duration, self.tick_duration, true);

                    timeout.start_time = Some(Instant::now());
                    timeout.tick_duration = Some(self.tick_duration);

                    let mut time_wheel = self.time_wheel.get_mut();

                    let slot = time_wheel.time_wheel.add(ticks, handle.token);

                    timeout.slot = Some(slot);

                    log::trace!(
                        "{:?} register as {:?}, slot={}, tick_duration={:?}, ticks={}, time_wheel_ticks={}, time_wheel_steps={}",
                        handle.token,
                        timeout,
                        timeout.slot.unwrap(),
                        self.tick_duration,
                        ticks,
                        time_wheel.time_wheel.tick,
                        time_wheel.time_wheel.steps
                    );

                    Ok(())
                })
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("[MioDriver] invalid register source: {:?}", handle),
                ))
            }
        }
    }

    fn reregister(&self, handle: Handle, interests: Interest) -> io::Result<()> {
        let mut mio_interests = mio::Interest::READABLE.add(mio::Interest::WRITABLE);

        if !interests.contains(Interest::Writable) {
            mio_interests = mio_interests.remove(mio::Interest::WRITABLE).unwrap();
        }

        if !interests.contains(Interest::Readable) {
            mio_interests = mio_interests.remove(mio::Interest::READABLE).unwrap();
        }

        match handle.desc {
            crate::Description::File => todo!(),
            crate::Description::TcpListener => TypedHandle::<mio::net::TcpListener>::new(handle)
                .with_mut(|source| {
                    self.registry
                        .reregister(source, mio::Token(handle.token.0), mio_interests)
                }),
            crate::Description::TcpStream => TypedHandle::<mio::net::TcpStream>::new(handle)
                .with_mut(|source| {
                    self.registry
                        .reregister(source, mio::Token(handle.token.0), mio_interests)
                }),
            crate::Description::UdpSocket => TypedHandle::<mio::net::UdpSocket>::new(handle)
                .with_mut(|source| {
                    self.registry
                        .reregister(source, mio::Token(handle.token.0), mio_interests)
                }),
            crate::Description::Timeout => {
                TypedHandle::<MioTimeout>::new(handle).with_mut(|timeout| {
                    assert!(!timeout.duration.is_zero());

                    let mut time_wheel = self.time_wheel.get_mut();

                    if let Some(slot) = timeout.slot {
                        time_wheel.time_wheel.remove(slot, handle.token);
                    }

                    let t = duration_to_ticks(timeout.duration, self.tick_duration, true);

                    let slot = time_wheel.time_wheel.add(t, handle.token);

                    timeout.start_time = Some(Instant::now());

                    timeout.slot = Some(slot);
                    timeout.tick_duration = Some(self.tick_duration);

                    Ok(())
                })
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("[MioDriver] invalid register source: {:?}", handle),
                ));
            }
        }
    }
    fn deregister(&self, handle: Handle) -> io::Result<()> {
        match handle.desc {
            crate::Description::File => todo!(),
            crate::Description::TcpListener => TypedHandle::<mio::net::TcpListener>::new(handle)
                .with_mut(|source| self.registry.deregister(source)),
            crate::Description::TcpStream => TypedHandle::<mio::net::TcpStream>::new(handle)
                .with_mut(|source| self.registry.deregister(source)),
            crate::Description::UdpSocket => TypedHandle::<mio::net::UdpSocket>::new(handle)
                .with_mut(|source| self.registry.deregister(source)),
            crate::Description::Timeout => {
                TypedHandle::<MioTimeout>::new(handle).with_mut(|timeout| {
                    assert!(!timeout.duration.is_zero());

                    let mut time_wheel = self.time_wheel.get_mut();

                    if let Some(slot) = timeout.slot {
                        time_wheel.time_wheel.remove(slot, handle.token);
                    }

                    Ok(())
                })
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("[MioDriver] invalid register source: {:?}", handle),
                ))
            }
        }
    }
}

pub type STMioPoller = BasicMioPoller<STModel>;

pub type MTMioPoller = BasicMioPoller<MTModel>;
