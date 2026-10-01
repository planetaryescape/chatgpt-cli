//! Which process a PID is, so a stale PID file never gets a reused PID
//! signalled. The daemon records its start time next to its PID, and a
//! client signals the PID only while `ps` still reports that start time
//! (spotuify's `ps -o lstart` check).

use std::process::{Command, Stdio};

/// When process `pid` started, as `ps -o lstart=` prints it; `None` when
/// there's no such process or `ps` can't say.
pub fn process_start_time(pid: u32) -> Option<String> {
    ps_field(pid, "lstart")
}

/// One `ps -o <field>=` value for `pid`; `None` when there's no such
/// process or `ps` can't say.
pub fn ps_field(pid: u32, field: &str) -> Option<String> {
    let output = Command::new("ps")
        .args(["-o", &format!("{field}="), "-p", &pid.to_string()])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let started = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !started.is_empty()).then_some(started)
}

/// The PID file's contents: the PID, then its start time.
pub fn pid_file_contents(pid: u32, started: &str) -> String {
    format!("{pid}\n{started}\n")
}

/// The PID and start time a PID file records.
pub fn parse_pid_file(contents: &str) -> Option<(u32, Option<String>)> {
    let mut lines = contents.lines();
    let pid = lines.next()?.trim().parse().ok()?;
    let started = lines
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned);
    Some((pid, started))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_has_a_start_time_and_round_trips_through_the_pid_file() {
        let pid = std::process::id();
        let started = process_start_time(pid).expect("ps knows this process");
        assert_eq!(
            parse_pid_file(&pid_file_contents(pid, &started)),
            Some((pid, Some(started)))
        );
        assert_eq!(parse_pid_file("42\n"), Some((42, None)));
        assert_eq!(parse_pid_file("x"), None);
    }
}
