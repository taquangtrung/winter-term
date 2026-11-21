//! The Z.ai API key, kept in the system's secret store: the Secret Service
//! (GNOME Keyring, KWallet) on Linux, the Keychain on macOS, the Credential
//! Manager on Windows. Never in the config file, which is plain text.

use keyring::{Entry, Error};

// ========================================================================
// Constants
// ========================================================================

/// The service Winter's secrets are filed under.
const SERVICE: &str = "winter-terminal";

/// The account the Z.ai key is filed as.
const ZAI_ACCOUNT: &str = "zai-api-key";

// ========================================================================
// Functions
// ========================================================================

/// The Z.ai API key, when one is kept.
pub(crate) fn zai_key() -> Option<String> {
    let key = Entry::new(SERVICE, ZAI_ACCOUNT).ok()?.get_password().ok()?;
    let key = key.trim();
    (!key.is_empty()).then(|| key.to_string())
}

/// Keep `key` as the Z.ai API key, or forget the one kept when it is blank.
pub(crate) fn set_zai_key(key: &str) -> Result<(), String> {
    let entry = Entry::new(SERVICE, ZAI_ACCOUNT).map_err(|e| store_error(&e))?;
    let key = key.trim();
    let done = match key.is_empty() {
        true => entry.delete_credential(),
        false => entry.set_password(key),
    };
    match done {
        Ok(()) | Err(Error::NoEntry) => Ok(()),
        Err(e) => Err(store_error(&e)),
    }
}

/// What to say when the secret store refused.
fn store_error(error: &Error) -> String {
    format!("the system's secret store refused: {error}")
}
