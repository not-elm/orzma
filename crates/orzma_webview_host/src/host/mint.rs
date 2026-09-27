//! Minting of the host's random identifiers from the OS random source.

use crate::error::{WebviewHostError, WebviewHostResult};
use data_encoding::BASE32_NOPAD;
use orzma_vt::prelude::InstanceId;

/// A fresh random identifier in lowercase unpadded base32 (`a-z2-7`),
/// usable verbatim as a URL host.
pub(crate) fn random_base32() -> WebviewHostResult<String> {
    let mut spelled = BASE32_NOPAD.encode(&random_bytes()?);
    spelled.make_ascii_lowercase();
    Ok(spelled)
}

/// A fresh placement instance.
pub(crate) fn mint_instance_id() -> WebviewHostResult<InstanceId> {
    Ok(InstanceId::from_bytes(random_bytes()?))
}

fn random_bytes() -> WebviewHostResult<[u8; 16]> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(WebviewHostError::Csprng)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boundary::HandleId;

    /// Asserts that every minted handle is a non-empty run of lowercase
    /// `a-z2-7`, safe to use verbatim as the host of an `orzma://` URL.
    ///
    /// Case: a program registers a view and the host mints the handle that
    /// becomes that registration's origin, which Chromium lowercases.
    #[test]
    fn minted_handles_are_lowercase_url_host_safe() {
        for _ in 0..50 {
            let handle = HandleId::mint().expect("the OS random source works");
            let spelled = handle.as_str();
            assert!(!spelled.is_empty() && spelled.len() <= 128);
            assert!(
                spelled
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || ('2'..='7').contains(&c)),
                "{spelled} must be lowercase base32"
            );
        }
    }

    /// Asserts that two mints never return the same value.
    ///
    /// Case: two programs register views at the same moment.
    #[test]
    fn minted_ids_are_unique() {
        assert_ne!(
            random_base32().expect("mints"),
            random_base32().expect("mints")
        );
        assert_ne!(
            mint_instance_id().expect("mints"),
            mint_instance_id().expect("mints")
        );
    }
}
