use std::sync::PoisonError;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum LGError {
    #[error("Encountered error during host communication: {0}")]
    LGMPCommunicationError(#[from] ligmars::error::Error),
    #[error("Failed to open SHM device due to error {0}")]
    SHMDeviceError(#[from] shared_memory::ShmemError),
    #[error("A thread panicked whilst holing lock on LGMP client")]
    LGMPClientLockPoisonError,
    #[error("The host appication is not compatible with this client; Expected KVMFR version {0}")]
    KVMFRVersionMismatch(u32),
    #[error("Message recieved from host on frame channel was smaller than expected")]
    FrameChannelMessageTooSmall,
    #[error("Message recieved from host on cursor channel was smaller than expected")]
    CursorChannelMessageTooSmall,
}

impl<T> From<PoisonError<T>> for LGError {
    fn from(_: PoisonError<T>) -> Self {
        Self::LGMPClientLockPoisonError
    }
}
