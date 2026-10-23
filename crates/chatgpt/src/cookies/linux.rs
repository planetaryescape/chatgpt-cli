//! Chromium's Linux Secret Service and cookie encryption formats.

use std::io::{ErrorKind, Read};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use sha2::{Digest, Sha256};

use super::{Browser, CookieError};

const KEYRING_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) fn default_browser() -> Option<Browser> {
    default_browser_with(&mut Command::new("xdg-mime"))
}

fn default_browser_with(command: &mut Command) -> Option<Browser> {
    let output = command_output(
        command.args(["query", "default", "x-scheme-handler/https"]),
        Duration::from_secs(5),
    )
    .ok()?;
    desktop_handler(std::str::from_utf8(&output).ok()?.trim())
}

fn desktop_handler(handler: &str) -> Option<Browser> {
    match handler {
        "chromium.desktop" | "chromium-browser.desktop" => Some(Browser::Chromium),
        _ => None,
    }
}

pub(super) fn keyring_password(_service: &str) -> Result<String, CookieError> {
    keyring_password_with(&mut Command::new("secret-tool"), KEYRING_TIMEOUT)
}

fn keyring_password_with(command: &mut Command, timeout: Duration) -> Result<String, CookieError> {
    let output = command_output(command.args(["lookup", "application", "chromium"]), timeout)
        .map_err(CookieError::SecretService)?;
    let password = String::from_utf8(output)
        .map_err(|_| CookieError::SecretService("the secret is not UTF-8"))?;
    // Captured secret-tool output has no added newline; preserve the secret bytes.
    if password.is_empty() {
        return Err(CookieError::SecretService(
            "no existing secret was returned",
        ));
    }
    Ok(password)
}

/// Captures stdout in memory, discards stderr, and always reaps the child.
/// Nonblocking reads keep both a stuck lookup and a full output pipe bounded.
fn command_output(command: &mut Command, timeout: Duration) -> Result<Vec<u8>, &'static str> {
    let (mut reader, writer) = UnixStream::pair().map_err(|_| "could not capture output")?;
    reader
        .set_nonblocking(true)
        .map_err(|_| "could not capture output")?;
    let mut child = command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::from(OwnedFd::from(writer)))
        .spawn()
        .map_err(|error| {
            if error.kind() == ErrorKind::NotFound {
                "command not found"
            } else {
                "could not start command"
            }
        })?;
    // Command retains its configured stdout handle; drop it so EOF can be observed.
    command.stdout(Stdio::null());
    let deadline = Instant::now() + timeout;
    let mut output = Vec::new();
    let mut status = None;
    let result = 'wait: loop {
        if Instant::now() >= deadline {
            break Err("command timed out");
        }
        if status.is_none() {
            status = match child.try_wait() {
                Ok(status) => status,
                Err(_) => break Err("could not wait for command"),
            };
        }
        let mut buffer = [0; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if output.len() + count > 65_536 {
                        break 'wait Err("command returned too much output");
                    }
                    output.extend_from_slice(&buffer[..count]);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => break 'wait Err("could not capture output"),
            }
        }
        if let Some(status) = status {
            break if status.success() {
                Ok(output)
            } else {
                Err("command failed or access was cancelled")
            };
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if result.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    result
}

pub(super) fn derive_key(password: &str) -> [u8; 16] {
    pbkdf2::pbkdf2_hmac_array::<sha1::Sha1, 16>(password.as_bytes(), b"saltysalt", 1)
}

#[derive(Default)]
pub(super) struct Decryptor {
    secret_key: Option<[u8; 16]>,
}

impl Decryptor {
    pub(super) fn decrypt(
        &mut self,
        encrypted: &[u8],
        host: &str,
        has_host_hash: bool,
        read_password: &dyn Fn(&str) -> Result<String, CookieError>,
    ) -> Result<String, CookieError> {
        let key = match encrypted.get(..3) {
            Some(b"v10") => derive_key("peanuts"),
            Some(b"v11") => match self.secret_key {
                Some(key) => key,
                None => *self
                    .secret_key
                    .insert(derive_key(&read_password("chromium")?)),
            },
            _ => {
                return Err(CookieError::Decrypt(
                    "unsupported Linux cookie encryption version",
                ));
            }
        };
        let mut buffer = encrypted[3..].to_vec();
        let plain = cbc::Decryptor::<aes::Aes128>::new((&key).into(), &[b' '; 16].into())
            .decrypt_padded_mut::<Pkcs7>(&mut buffer)
            .map_err(|_| {
                CookieError::Decrypt("bad padding; the Chromium cookie key may be wrong")
            })?;
        let plain = if has_host_hash {
            let (hash, value) = plain
                .split_at_checked(32)
                .ok_or(CookieError::Decrypt("value is shorter than its host hash"))?;
            if hash != Sha256::digest(host.as_bytes()).as_slice() {
                return Err(CookieError::Decrypt(
                    "host hash mismatch; the Chromium cookie key or host may be wrong",
                ));
            }
            value
        } else {
            plain
        };
        String::from_utf8(plain.to_vec()).map_err(|_| CookieError::Decrypt("value is not UTF-8"))
    }
}

#[cfg(test)]
mod command_tests;
#[cfg(test)]
mod tests;
