#![allow(clippy::unwrap_used)]

use std::os::unix::fs::PermissionsExt;

use super::*;

fn script(body: &str) -> (tempfile::TempDir, Command) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stand-in");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    (dir, Command::new(path))
}

#[test]
fn invokes_xdg_mime_with_exact_arguments_and_rejects_failed_or_unknown_handlers() {
    let (_dir, mut command) = script(
        "[ \"$#\" = 3 ] && [ \"$1\" = query ] && [ \"$2\" = default ] && [ \"$3\" = x-scheme-handler/https ] || exit 9\nprintf 'chromium.desktop\\n'",
    );
    assert_eq!(default_browser_with(&mut command), Some(Browser::Chromium));
    for body in [
        "printf 'firefox.desktop\\n'",
        "printf 'chromium.desktop\\n'; exit 1",
        "exit 0",
    ] {
        let (_dir, mut command) = script(body);
        assert_eq!(default_browser_with(&mut command), None);
    }
}

#[test]
fn looks_up_only_existing_chromium_secret_and_preserves_password_spaces() {
    let (_dir, mut command) = script(
        "[ \"$#\" = 3 ] && [ \"$1\" = lookup ] && [ \"$2\" = application ] && [ \"$3\" = chromium ] || exit 9\nprintf ' secret with spaces '",
    );
    assert_eq!(
        keyring_password_with(&mut command, Duration::from_secs(1)).unwrap(),
        " secret with spaces "
    );
}

#[test]
fn captured_secret_preserves_embedded_and_trailing_newlines() {
    let (_dir, mut command) = script("printf 'secret\\nwith newline\\n'");
    assert_eq!(
        keyring_password_with(&mut command, Duration::from_secs(1)).unwrap(),
        "secret\nwith newline\n"
    );
}

#[test]
fn missing_failed_empty_invalid_and_timedout_keyring_errors_are_secret_free() {
    let dir = tempfile::tempdir().unwrap();
    let error = keyring_password_with(
        &mut Command::new(dir.path().join("missing-secret-tool")),
        Duration::from_secs(1),
    )
    .unwrap_err();
    assert!(error.to_string().contains("command not found"));
    for (body, expected) in [
        (
            "printf 'PRIVATE-SENTINEL'; printf 'PRIVATE-STDERR' >&2; exit 1",
            "command failed",
        ),
        ("exit 0", "no existing secret"),
        ("printf '\\377'", "not UTF-8"),
        ("exec /bin/sleep 30", "timed out"),
    ] {
        let (_dir, mut command) = script(body);
        let started = Instant::now();
        let timeout = if expected == "timed out" {
            Duration::from_millis(100)
        } else {
            Duration::from_secs(1)
        };
        let error = keyring_password_with(&mut command, timeout).unwrap_err();
        let printed = format!("{error} {error:?}");
        assert!(printed.contains(expected), "{printed}");
        assert!(!printed.contains("PRIVATE-SENTINEL"));
        assert!(!printed.contains("PRIVATE-STDERR"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}

#[test]
fn oversized_output_is_bounded_and_stderr_is_not_captured() {
    let (_dir, mut command) = script("exec /bin/dd if=/dev/zero bs=4096 count=32");
    let error = keyring_password_with(&mut command, Duration::from_secs(2)).unwrap_err();
    assert!(error.to_string().contains("too much output"));
    let (_dir, mut command) =
        script("/bin/dd if=/dev/zero bs=4096 count=32 >&2\nprintf 'synthetic-secret'");
    assert_eq!(
        keyring_password_with(&mut command, Duration::from_secs(2)).unwrap(),
        "synthetic-secret"
    );
}

#[test]
fn timedout_child_is_killed_and_reaped() {
    let (_dir, mut command) = script("printf '%s\\n' \"$$\" > \"$PID_FILE\"\nexec /bin/sleep 30");
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    command.env("PID_FILE", &pid_file);
    assert!(matches!(
        keyring_password_with(&mut command, Duration::from_secs(1)),
        Err(CookieError::SecretService("command timed out"))
    ));
    let pid = std::fs::read_to_string(pid_file).unwrap();
    let status = Command::new("/bin/kill")
        .args(["-0", pid.trim()])
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(
        !status.success(),
        "timed-out child is still alive or unreaped"
    );
}
