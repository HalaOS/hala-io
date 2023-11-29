use std::io;

use crate::driver::{CtlOps, Driver, FileDescription, Handle, Interest};

#[derive(Clone)]
pub struct File {
    pub driver: Driver,
    pub handle: Handle,
    pub poller: Handle,
}

impl File {
    pub fn new(
        driver: Driver,
        poller: Handle,
        path: &str,
        interests: Interest,
    ) -> io::Result<Self> {
        let handle = driver.fd_open(FileDescription::File)?;

        driver.fd_ctl(handle, CtlOps::OpenFile(path))?;

        driver.fd_ctl(
            poller,
            CtlOps::Register {
                handles: &[handle],
                interests,
            },
        )?;

        Ok(Self {
            handle,
            poller,
            driver,
        })
    }
}
