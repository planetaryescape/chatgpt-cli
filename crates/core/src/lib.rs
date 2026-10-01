//! What every chatgpt crate shares: where files live
//! ([`Paths`], [`Instance`]), the categories of failure ([`ErrorKind`]),
//! and where the TS CLI that unported commands go to lives ([`ts_cli`]).
//! No I/O beyond reading paths and the environment.

mod duration;
mod error;
pub mod legacy;
mod paths;
pub mod ts_cli;

pub use duration::format_duration;
pub use error::{ErrorKind, message_with_causes};
pub use paths::{
    APP_NAME, DAEMON_LOG_PREFIX, INSTANCE_ENV, Instance, InvalidInstanceName, Paths, PathsError,
};
