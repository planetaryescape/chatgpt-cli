// Adapted from ms-todo crates/core/src/error.rs @ 72a406042a4d46c2f8cc7c3afe6bda12e11f923b.
// Changes: chatgpt's kinds (no outbox or Graph sign-in kinds; `not_synced`
// for the TS CLI's "No local index yet").

/// The category of a failure, shared by every crate so the CLI can map any
/// error to the same `kind` string and exit code. The daemon sends the `kind`
/// string over IPC, so the strings are part of the protocol.
///
/// Every failure exits non-zero, as every TS CLI failure exits 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// Bad arguments: a filter, a limit, a format.
    InvalidInput,
    /// No browser session to read, or the session expired.
    AuthRequired,
    /// ChatGPT kept answering 429, or asked for a long pause.
    RateLimited,
    /// This platform or build can't do it.
    Unsupported,
    /// The request never got a usable answer: connection, TLS, Cloudflare.
    Network,
    /// ChatGPT answered with an error after the retries.
    Api,
    /// A response body didn't have the shape we expected.
    Decode,
    /// The index has never been synced: "No local index yet".
    NotSynced,
    /// The daemon couldn't be started or reached, or speaks another protocol.
    DaemonUnavailable,
    /// A local failure: the database, a file, a bug.
    Internal,
    /// The index was upgraded by a newer chatgpt, so this one can't read it.
    DatabaseTooNew,
}

impl ErrorKind {
    pub const ALL: [Self; 11] = [
        Self::InvalidInput,
        Self::AuthRequired,
        Self::RateLimited,
        Self::Unsupported,
        Self::Network,
        Self::Api,
        Self::Decode,
        Self::NotSynced,
        Self::DaemonUnavailable,
        Self::Internal,
        Self::DatabaseTooNew,
    ];

    /// The stable `kind` value on the IPC wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::AuthRequired => "auth_required",
            Self::RateLimited => "rate_limited",
            Self::Unsupported => "unsupported",
            Self::Network => "network",
            Self::Api => "api",
            Self::Decode => "decode",
            Self::NotSynced => "not_synced",
            Self::DaemonUnavailable => "daemon_unavailable",
            Self::Internal => "internal",
            Self::DatabaseTooNew => "database_too_new",
        }
    }

    /// The inverse of [`Self::as_str`]. `None` for a kind this build doesn't
    /// know, such as one a newer daemon sent.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }

    /// The process exit code: ms-todo's table. Anything the TS CLI exits 1
    /// for stays non-zero.
    pub fn exit_code(self) -> u8 {
        match self {
            Self::InvalidInput => 2,
            Self::AuthRequired => 4,
            Self::RateLimited => 6,
            Self::Unsupported => 7,
            Self::Network
            | Self::Api
            | Self::Decode
            | Self::NotSynced
            | Self::DaemonUnavailable
            | Self::Internal
            | Self::DatabaseTooNew => 1,
        }
    }
}

/// An error's message followed by each of its causes.
pub fn message_with_causes(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

#[cfg(test)]
mod tests {
    use super::ErrorKind;

    #[test]
    fn every_kind_exits_non_zero() {
        for kind in ErrorKind::ALL {
            assert_ne!(kind.exit_code(), 0, "{}", kind.as_str());
        }
        assert_eq!(ErrorKind::InvalidInput.exit_code(), 2);
        assert_eq!(ErrorKind::AuthRequired.exit_code(), 4);
        assert_eq!(ErrorKind::RateLimited.exit_code(), 6);
    }

    #[test]
    fn kind_strings_round_trip_and_are_unique() {
        for kind in ErrorKind::ALL {
            assert_eq!(ErrorKind::parse(kind.as_str()), Some(kind));
        }
        let mut strings: Vec<_> = ErrorKind::ALL.iter().map(|kind| kind.as_str()).collect();
        strings.sort_unstable();
        strings.dedup();
        assert_eq!(strings.len(), ErrorKind::ALL.len());
        assert_eq!(ErrorKind::parse("from_a_newer_daemon"), None);
    }
}
