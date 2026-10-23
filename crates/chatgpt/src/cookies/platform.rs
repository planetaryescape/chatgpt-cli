//! Browser selection and roots are explicit so platform fixtures run on every host.

use std::path::{Path, PathBuf};

use super::{Browser, CookieError};

pub(super) const MACOS_BROWSERS: [Browser; 7] = [
    Browser::Dia,
    Browser::Chrome,
    Browser::Safari,
    Browser::Firefox,
    Browser::Arc,
    Browser::Brave,
    Browser::Edge,
];
pub(super) const LINUX_BROWSERS: [Browser; 1] = [Browser::Chromium];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Platform {
    MacOs,
    Linux,
    Unsupported,
}

impl Platform {
    pub(super) const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Unsupported
        }
    }

    pub(super) const fn browsers(self) -> &'static [Browser] {
        match self {
            Self::MacOs => &MACOS_BROWSERS,
            Self::Linux => &LINUX_BROWSERS,
            Self::Unsupported => &[],
        }
    }

    pub(super) fn select(self, name: &str) -> Result<Browser, CookieError> {
        self.browsers()
            .iter()
            .copied()
            .find(|browser| browser.name() == name)
            .ok_or_else(|| CookieError::UnsupportedBrowser(name.to_owned()))
    }
}

pub(super) fn linux_root(home: &Path, config_home: Option<&Path>) -> PathBuf {
    config_home
        .filter(|path| path.is_absolute())
        .map(Path::to_owned)
        .unwrap_or_else(|| home.join(".config"))
        .join("chromium")
}

const LAUNCH_SERVICES_PLIST: &str =
    "Library/Preferences/com.apple.LaunchServices/com.apple.launchservices.secure.plist";

/// The macOS default browser: the https handler with the newest modification date.
pub(super) fn macos_default_browser(home: &Path) -> Option<Browser> {
    let plist = plist::Value::from_file(home.join(LAUNCH_SERVICES_PLIST)).ok()?;
    default_browser_from_plist(&plist)
}

pub(super) fn default_browser_from_plist(plist: &plist::Value) -> Option<Browser> {
    let handlers = plist.as_dictionary()?.get("LSHandlers")?.as_array()?;
    let modified = |handler: &plist::Dictionary| {
        handler
            .get("LSHandlerModificationDate")
            .and_then(|date| {
                date.as_real()
                    .or_else(|| date.as_signed_integer().map(|n| n as f64))
            })
            .unwrap_or(0.0)
    };
    handlers
        .iter()
        .filter_map(plist::Value::as_dictionary)
        .filter(|handler| {
            handler
                .get("LSHandlerURLScheme")
                .and_then(plist::Value::as_string)
                == Some("https")
        })
        // max_by keeps the last of equal maxima; reversing keeps the first,
        // matching the TS stable sort when dates tie or are missing.
        .rev()
        .max_by(|a, b| modified(a).total_cmp(&modified(b)))
        .and_then(|handler| handler.get("LSHandlerRoleAll")?.as_string())
        .and_then(Browser::for_bundle_id)
}
