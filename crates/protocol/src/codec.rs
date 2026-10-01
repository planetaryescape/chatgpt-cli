// Adapted from ms-todo crates/protocol/src/codec.rs @ 72a406042a4d46c2f8cc7c3afe6bda12e11f923b
// (itself from mxr crates/protocol/src/codec.rs). Unchanged apart from names.

use bytes::BytesMut;
use tokio_util::codec::{Decoder, Encoder, LengthDelimitedCodec};

use crate::Message;

/// The largest frame either side sends or accepts, set explicitly rather than
/// relying on tokio-util's 8 MiB default. `list --json` over every chat,
/// Jev's answers included, is about 1 MiB for 800 chats.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// Frames [`Message`]s on the daemon socket: a 4-byte big-endian length, then
/// that many bytes of JSON. Malformed JSON and oversized frames surface as
/// `io::ErrorKind::InvalidData`.
pub struct Codec {
    inner: LengthDelimitedCodec,
}

impl Codec {
    pub fn new() -> Self {
        Self::with_max_frame(MAX_FRAME_BYTES)
    }

    /// A smaller cap, so tests don't need 16 MiB frames.
    #[doc(hidden)]
    pub fn with_max_frame(max_frame_bytes: usize) -> Self {
        Self {
            inner: LengthDelimitedCodec::builder()
                .length_field_length(4)
                .max_frame_length(max_frame_bytes)
                .new_codec(),
        }
    }
}

impl Default for Codec {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder for Codec {
    type Item = Message;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self.inner.decode(src)? {
            Some(frame) => serde_json::from_slice(&frame)
                .map(Some)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
            None => Ok(None),
        }
    }
}

impl Encoder<Message> for Codec {
    type Error = std::io::Error;

    fn encode(&mut self, item: Message, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let json = serde_json::to_vec(&item)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let cap = self.inner.max_frame_length();
        if json.len() > cap {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                FrameTooLarge {
                    bytes: json.len(),
                    cap,
                },
            ));
        }
        self.inner.encode(json.into(), dst)
    }
}

/// An outgoing message was bigger than the frame cap. The daemon catches
/// this and answers with an error response instead.
#[derive(Debug, thiserror::Error)]
#[error("message of {bytes} bytes is over the {cap} byte IPC frame cap")]
pub struct FrameTooLarge {
    pub bytes: usize,
    pub cap: usize,
}
