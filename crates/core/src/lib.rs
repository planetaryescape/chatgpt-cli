//! What every chatgpt crate shares: where files live
//! ([`Paths`], [`Instance`]), the categories of failure ([`ErrorKind`]),
//! and where the TS CLI that unported commands go to lives ([`ts_cli`]).
//! No I/O beyond reading paths and the environment, reading and writing
//! the user config ([`user_config`]), and handing text or a link to
//! macOS's `pbcopy` and `open` ([`desktop`]).

pub mod desktop;
mod duration;
mod error;
pub mod js;
mod js_number;
pub mod legacy;
mod paths;
mod process;
pub mod ts_cli;
pub mod user_config;

pub use duration::format_duration;

/// A test-only setting from the environment: in debug builds, `name`'s
/// value when set and not empty; never in a release build.
pub fn debug_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| cfg!(debug_assertions) && !value.is_empty())
}
pub use error::ErrorKind;
pub use js_number::js_number_string;
pub use paths::{
    APP_NAME, DAEMON_LOG_PREFIX, INSTANCE_ENV, Instance, InvalidInstanceName, Paths, PathsError,
};
pub use process::{parse_pid_file, pid_file_contents, process_start_time, ps_field};
