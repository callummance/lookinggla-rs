use std::{
    mem::{size_of, size_of_val, transmute},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use ligmars::client::{Client, InPlaceMessage};

use crate::{error::LGError, shm_datastructs};

#[derive(Clone)]
pub struct LGMPOpts {
    pub shm_path: String,
    pub timeout: Duration,
}

pub struct LGMPConnection {
    client: Arc<Mutex<ligmars::client::Client>>,
    session: Option<LGMPSession>,
    opts: LGMPOpts,
}

impl LGMPConnection {
    /// Creates a new LGMP client handle, but does not register it with the
    /// host or start listening.
    ///
    /// After calling this,
    pub fn open(opts: LGMPOpts) -> Result<LGMPConnection, LGError> {
        let shm_file = shared_memory::ShmemConf::new()
            .flink(&opts.shm_path)
            .open()?;
        let client = Client::init(Box::new(shm_file))?;

        Ok(LGMPConnection {
            client: Arc::new(Mutex::new(client)),
            session: None,
            opts,
        })
    }

    /// Initialises a client session.
    ///
    /// Note that this must not be called immediately after the connection is created;
    /// due to one of the safety checks in the underlying C library, it is recommended
    /// to wait around 200ms before calling init.
    ///
    /// Calling this will also cause the host to begin tracking timeouts on this client.
    pub fn init(&mut self) -> Result<(), LGError> {
        let mut client = self.client.lock()?;
        //Init client session
        let (udata_raw, _client_id) = client.client_session_init()?;
        //Version checks
        if udata_raw.len() != size_of::<shm_datastructs::KVMFR>() {
            Err(LGError::KVMFRVersionMismatch(
                shm_datastructs::KVMFR_VERSION,
            ))?
        }
        let udata: &shm_datastructs::KVMFR =
            unsafe { &*(udata_raw.as_ptr() as *const shm_datastructs::KVMFR) };
        let magic: &[u8] = unsafe { &*(&udata.magic as *const [i8] as *const [u8]) };
        if magic != &shm_datastructs::KVMFR_MAGIC[0..udata.magic.len()]
            || udata.version != shm_datastructs::KVMFR_VERSION
        {
            Err(LGError::KVMFRVersionMismatch(
                shm_datastructs::KVMFR_VERSION,
            ))?
        }

        //Subscribe to channels
        let frame_chan = client.client_subscribe(shm_datastructs::LGMP_Q_FRAME)?;
        let cursor_chan = client.client_subscribe(shm_datastructs::LGMP_Q_POINTER)?;

        //Set timeouts
        let now = Instant::now();
        let last_frame_heartbeat = now - self.opts.timeout;
        let last_cursor_heartbeat = now - self.opts.timeout;

        //Session struct
        let session = LGMPSession {
            frame_chan,
            cursor_chan,
            last_frame_heartbeat,
            last_cursor_heartbeat,
        };

        self.session = Some(session);

        Ok(())
    }

    /// Attempts to avoid the LGMP client being timed out by the host by fast-forwarding
    /// the frame queue if we think that any messages are likely to be timed out before
    /// this function is next called.
    /// If a session has not yet been initialised, this function will do nothing.
    ///
    /// Specifically, messages will be skipped if we have not completely emptied out the
    /// queue recently.
    ///
    /// It is recommended that this function be called around every 1ms.
    pub fn tick_frame(&mut self, tick_period: Duration) -> Result<(), LGError> {
        if let Some(ref mut sess) = self.session {
            let projected_timeout = sess.last_frame_heartbeat + self.opts.timeout;
            if Instant::now() + tick_period > projected_timeout {
                sess.fast_forward(KVMFRChans::Frame)?;
                sess.last_frame_heartbeat = Instant::now();
            }
        }
        Ok(())
    }

    /// See [tick_frame]
    ///
    /// It is recommended that this function be called around every 1ms.
    pub fn tick_cursor(&mut self, tick_period: Duration) -> Result<(), LGError> {
        if let Some(ref mut sess) = self.session {
            let projected_timeout = sess.last_cursor_heartbeat + self.opts.timeout;
            if Instant::now() + tick_period > projected_timeout {
                sess.fast_forward(KVMFRChans::Cursor)?;
                sess.last_cursor_heartbeat = Instant::now();
            }
        }
        Ok(())
    }

    /// Retrieves an update from the frame channel if one is available, returning a handle
    /// to it if so. The channel will remain locked until this value is dropped.
    pub fn get_frame_update(&mut self) -> Result<Option<KVMFRFrameHandle>, LGError> {
        if let Some(ref mut sess) = self.session {
            let msg = sess.pop_ref(KVMFRChans::Frame)?;
            Ok(msg.map(|m| KVMFRFrameHandle { _msg_handle: m }))
        } else {
            Ok(None)
        }
    }

    /// Retrieves an update from the cursor channel if one is available, returning a handle
    /// to it if so. The channel will remain locked until this value is dropped.
    pub fn get_cursor_update(&mut self) -> Result<Option<KVMFRCursorHandle>, LGError> {
        if let Some(ref mut sess) = self.session {
            let msg = sess.pop_ref(KVMFRChans::Cursor)?;
            Ok(msg.map(|m| KVMFRCursorHandle { _msg_handle: m }))
        } else {
            Ok(None)
        }
    }
}

pub struct KVMFRFrameHandle<'a> {
    _msg_handle: InPlaceMessage<'a>,
}

impl KVMFRFrameHandle<'_> {
    pub fn as_frame<'a>(&'a self) -> Result<&'a shm_datastructs::KVMFRFrame, LGError> {
        let msg = &self._msg_handle.mem;
        if msg.size < size_of::<shm_datastructs::KVMFRFrame>() {
            Err(LGError::FrameChannelMessageTooSmall)
        } else {
            let ptr = msg.mem;
            let res = unsafe { &*ptr.cast::<shm_datastructs::KVMFRFrame>() };
            Ok(res)
        }
    }
}

pub struct KVMFRCursorHandle<'a> {
    _msg_handle: InPlaceMessage<'a>,
}

impl KVMFRCursorHandle<'_> {
    pub fn as_ptr_msg<'a>(&'a self) -> Result<&'a shm_datastructs::KVMFRCursor, LGError> {
        let msg = &self._msg_handle.mem;
        if msg.size < size_of::<shm_datastructs::KVMFRCursor>() {
            Err(LGError::CursorChannelMessageTooSmall)
        } else {
            let ptr = msg.mem;
            let res = unsafe { &*ptr.cast::<shm_datastructs::KVMFRCursor>() };
            Ok(res)
        }
    }
}

/// Selector for the channels subscribed to by LGMP client
enum KVMFRChans {
    Frame,
    Cursor,
}

/// Holds handles to channels listened to by an LGMP client, as well as the
/// times at which they last received an LGMPErrQueueEmpty response.
struct LGMPSession {
    frame_chan: ligmars::client::ClientQueueHandle,
    cursor_chan: ligmars::client::ClientQueueHandle,

    last_frame_heartbeat: Instant,
    last_cursor_heartbeat: Instant,
}

impl LGMPSession {
    /// Returns a refererence to the data contained within the next message in the
    /// requested channel. This reference also holds a lock on the channel.
    ///
    /// If the channel is empty, returns Ok(None)
    fn pop_ref(&mut self, channel: KVMFRChans) -> Result<Option<InPlaceMessage>, LGError> {
        let (chan, hb) = match channel {
            KVMFRChans::Frame => (&mut self.frame_chan, &mut self.last_frame_heartbeat),
            KVMFRChans::Cursor => (&mut self.cursor_chan, &mut self.last_cursor_heartbeat),
        };

        let msg = match chan.pop_in_place() {
            Ok(msg) => Ok(Some(msg)),
            Err(ligmars::error::Error::InternalError(
                ligmars::error::Status::LGMPErrQueueEmpty,
            )) => {
                *hb = Instant::now();
                Ok(None)
            }
            Err(e) => Err(e),
        }?;

        Ok(msg)
    }

    /// Marks all but the most recent message in a channel as read.
    fn fast_forward(&mut self, channel: KVMFRChans) -> Result<(), LGError> {
        let (chan, hb) = match channel {
            KVMFRChans::Frame => (&mut self.frame_chan, &mut self.last_frame_heartbeat),
            KVMFRChans::Cursor => (&mut self.cursor_chan, &mut self.last_cursor_heartbeat),
        };

        chan.advance_to_last().or_else(|e| match e {
            ligmars::error::Error::InternalError(ligmars::error::Status::LGMPErrQueueEmpty) => {
                *hb = Instant::now();
                Ok(())
            }
            e => Err(e)?,
        })
    }
}
