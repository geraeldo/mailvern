//! Account passwords live in the macOS Keychain, never in the database.

use anyhow::{anyhow, Result};

const SERVICE: &str = "Mailvern";

pub fn get(email: &str) -> Result<String> {
    keyring::Entry::new(SERVICE, email)?
        .get_password()
        .map_err(|e| anyhow!("No password in Keychain for {email}: {e}"))
}

pub fn set(email: &str, password: &str) -> Result<()> {
    keyring::Entry::new(SERVICE, email)?.set_password(password)?;
    Ok(())
}

pub fn delete(email: &str) {
    if let Ok(e) = keyring::Entry::new(SERVICE, email) {
        let _ = e.delete_credential();
    }
}
