//! What every chatgpt crate shares: where files live
//! ([`Paths`], [`Instance`]), the categories of failure ([`ErrorKind`]),
//! and where the TS CLI that unported commands go to lives ([`ts_cli`]).
//! No I/O beyond reading paths and the environment.

mod duration;
mod error;
mod js_number;
pub mod legacy;
mod paths;
mod process;
pub mod ts_cli;

pub use duration::format_duration;
pub use error::ErrorKind;
pub use js_number::js_number_string;
pub use paths::{
    APP_NAME, DAEMON_LOG_PREFIX, INSTANCE_ENV, Instance, InvalidInstanceName, Paths, PathsError,
};
pub use process::{parse_pid_file, pid_file_contents, process_start_time, ps_field};
