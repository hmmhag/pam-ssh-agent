pub use crate::agent::SSHAgent;
use crate::verify::verify;
use anyhow::{anyhow, Context, Result};
use log::debug;
use ssh_agent_client_rs::{Error as SACError, Identity};
use ssh_key::public::KeyData;
use ssh_key::AuthorizedKeys;
use std::collections::HashSet;
use std::path::Path;
use Identity::{Certificate, PublicKey};

const CHALLENGE_SIZE: usize = 32;

/// Finds the first key, if any, that the ssh-agent knows about that is also present
/// in the file referenced by keys_file_path, sends a random message to be signed and
/// verifies the signature with the public key.
///
/// Returns Ok(true) if a key was found and the signature was correct, Ok(false) if no
/// key was found, and Err if agent communication or signature verification failed.
pub fn authenticate(keys_file_path: &str, mut agent: impl SSHAgent) -> Result<bool> {
    let keys = keys_from_file(Path::new(keys_file_path))?;
    for identity in agent.list_identities()? {
        if filter_identities(&identity, &keys) {
            // Allow sign_and_verify() to return RemoteFailure (key not loaded / present),
            // and try the next configured key
            match sign_and_verify(identity, &mut agent) {
                Ok(res) => return Ok(res),
                Err(e) => {
                    if let Some(SACError::RemoteFailure) = e.downcast_ref::<SACError>() {
                        debug!("SSHAgent: RemoteFailure; trying next key");
                        continue;
                    } else {
                        return Err(e);
                    }
                }
            }
        }
    }
    Ok(false)
}

fn filter_identities(identity: &Identity, keys: &HashSet<KeyData>) -> bool {
    if let PublicKey(key) = identity {
        if keys.contains(key.key_data()) {
            debug!(
                "found a matching key: {}",
                key.fingerprint(Default::default())
            );
            return true;
        }
    }
    false
}

fn sign_and_verify(identity: Identity<'static>, agent: &mut impl SSHAgent) -> Result<bool> {
    let mut data: [u8; CHALLENGE_SIZE] = [0_u8; CHALLENGE_SIZE];
    getrandom::fill(data.as_mut_slice()).map_err(|_| anyhow!("Failed to obtain random data"))?;
    let sig = agent.sign(identity.clone(), data.as_ref())?;
    match identity {
        PublicKey(key) => verify(key.key_data(), data.as_ref(), &sig)?,
        Certificate(_) => return Err(anyhow!("Unsupported identity type")),
    };
    Ok(true)
}

fn keys_from_file(path: &Path) -> Result<HashSet<KeyData>> {
    Ok(HashSet::from_iter(
        AuthorizedKeys::read_file(path)
            .context(format!("Failed to read from {path:?}"))?
            .into_iter()
            .map(|e| e.public_key().key_data().to_owned()),
    ))
}

#[cfg(test)]
mod test {
    use crate::auth::keys_from_file;
    use ssh_key::PublicKey;
    use std::path::Path;

    const KEY_FROM_AUTHORIZED_KEYS: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIObUcR\
        y1Nv6fz4xnAXqOaFL/A+gGM9OF+l2qpsDPmMlU test@ed25519";

    const ANOTHER_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMdtbb2fnK02RReYsJW\
        jh1F2q102dIer60vbgj+cABcO noa@Noas-Laptop.local";

    #[test]
    fn test_read_public_keys() {
        let path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/authorized_keys"
        ));

        let result = keys_from_file(path).expect("Failed to parse");

        let key = PublicKey::from_openssh(KEY_FROM_AUTHORIZED_KEYS).unwrap();
        assert!(result.contains(key.key_data()));

        let key = PublicKey::from_openssh(ANOTHER_KEY).unwrap();
        assert!(!result.contains(key.key_data()));
    }
}
