//! The assistant's Anthropic API key in the system keychain (roadmap G22): the macOS Keychain,
//! the Windows Credential Manager or the Secret Service on Linux. A key in `ANTHROPIC_API_KEY`
//! still wins, for development; a Finder-launched app never sees it, so normal users paste
//! their key into the assistant panel once. The key is never written to the preferences, a
//! project or a log, and is sent only to Anthropic. The self-test uses an in-memory store so it
//! never touches the user's keychain.

use std::cell::RefCell;

const SERVICE: &str = "Device";
const ACCOUNT: &str = "anthropic-api-key";

/// Where the key is kept.
pub enum KeyStore {
    System,
    /// Tests: a key held in memory only.
    Memory(RefCell<Option<String>>),
}

impl KeyStore {
    fn entry() -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| e.to_string())
    }

    /// The saved key, if any (a keychain that can't be read counts as none).
    pub fn load(&self) -> Option<String> {
        match self {
            KeyStore::System => Self::entry().ok()?.get_password().ok().filter(|k| !k.trim().is_empty()),
            KeyStore::Memory(k) => k.borrow().clone(),
        }
    }

    pub fn save(&self, key: &str) -> Result<(), String> {
        match self {
            KeyStore::System => Self::entry()?.set_password(key).map_err(|e| e.to_string()),
            KeyStore::Memory(k) => {
                *k.borrow_mut() = Some(key.to_string());
                Ok(())
            }
        }
    }

    pub fn delete(&self) -> Result<(), String> {
        match self {
            KeyStore::System => match Self::entry()?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(e.to_string()),
            },
            KeyStore::Memory(k) => {
                *k.borrow_mut() = None;
                Ok(())
            }
        }
    }
}

/// Why `key` can't be an API key, if it can't: empty, or with spaces or line breaks in it.
pub fn check(key: &str) -> Result<(), &'static str> {
    if key.is_empty() {
        return Err("the key is empty");
    }
    if key.chars().any(char::is_whitespace) {
        return Err("the key has spaces or line breaks in it");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_with_spaces_are_refused_and_the_memory_store_round_trips() {
        assert!(check("sk-ant-abc123").is_ok());
        assert!(check("").is_err() && check("sk ant").is_err() && check("sk-ant\n").is_err());
        let s = KeyStore::Memory(RefCell::new(None));
        assert_eq!(s.load(), None);
        s.save("sk-ant-x").unwrap();
        assert_eq!(s.load().as_deref(), Some("sk-ant-x"));
        s.delete().unwrap();
        assert_eq!(s.load(), None);
    }
}
