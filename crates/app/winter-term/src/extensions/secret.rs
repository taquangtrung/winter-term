//! Extensions' secrets, kept in the system's secret store: the Secret
//! Service (GNOME Keyring, KWallet) on Linux, the Keychain on macOS, the
//! Credential Manager on Windows. Never in a file, and never handed to the
//! extension: Winter puts them in the requests that name them.
//!
//! Each is filed as `<extension>-<key>`, so one extension never reaches
//! another's.

use keyring::{Entry, Error};

// ========================================================================
// Constants
// ========================================================================

/// The service Winter's secrets are filed under.
const SERVICE: &str = "winter-terminal";

// ========================================================================
// Functions
// ========================================================================

/// The secret `key` of `extension`, when one is kept.
pub(crate) fn get(extension: &str, key: &str) -> Option<String> {
    let kept = Entry::new(SERVICE, &account(extension, key))
        .ok()?
        .get_password()
        .ok()?;
    let kept = kept.trim();
    (!kept.is_empty()).then(|| kept.to_string())
}

/// Keep `value` as the secret `key` of `extension`, or forget the one kept
/// when it is blank.
pub(crate) fn set(extension: &str, key: &str, value: &str) -> Result<(), String> {
    let entry = Entry::new(SERVICE, &account(extension, key)).map_err(|e| store_error(&e))?;
    let value = value.trim();
    let done = match value.is_empty() {
        true => entry.delete_credential(),
        false => entry.set_password(value),
    };
    match done {
        Ok(()) | Err(Error::NoEntry) => Ok(()),
        Err(e) => Err(store_error(&e)),
    }
}

/// The account `extension`'s secret `key` is filed as.
fn account(extension: &str, key: &str) -> String {
    format!("{extension}-{key}")
}

/// What to say when the secret store refused.
fn store_error(error: &Error) -> String {
    format!("the system's secret store refused: {error}")
}
