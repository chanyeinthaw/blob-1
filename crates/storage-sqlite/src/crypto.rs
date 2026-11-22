use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::Argon2;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chacha20poly1305::{
    aead::{Aead, AeadCore, KeyInit},
    XChaCha20Poly1305, XNonce,
};
use rusqlite::{params, Connection, OptionalExtension};
use storage::StoreError;

use crate::{backend, SqliteStorage};

const PREFIX: &str = "enc:v1:";
/// Fixed plaintext encrypted into `app_key.canary`; decrypting it proves the
/// passphrase matches this database.
const CANARY: &[u8] = b"blob-1 app key";
const NO_KEY: &str = "set BLOB_1_APP_KEY";

/// The `BLOB_1_APP_KEY` passphrase; any non-empty string. It derives a
/// per-database key (argon2id over a random stored salt) that encrypts
/// secrets at rest; the passphrase itself is never stored.
pub struct AppKey(String);

impl AppKey {
    pub fn new(passphrase: String) -> Self {
        Self(passphrase)
    }

    fn derive(&self, salt: &[u8]) -> Result<[u8; 32], StoreError> {
        let mut key = [0u8; 32];
        Argon2::default()
            .hash_password_into(self.0.as_bytes(), salt, &mut key)
            .map_err(|error| StoreError::Backend(error.to_string().into()))?;
        Ok(key)
    }
}

fn cipher(key: &[u8; 32]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new_from_slice(key).expect("key is 32 bytes")
}

fn encrypt(cipher: &XChaCha20Poly1305, plaintext: &[u8]) -> Result<Vec<u8>, StoreError> {
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|_| StoreError::Backend("encryption failed".into()))?;
    let mut value = nonce.to_vec();
    value.extend_from_slice(&ciphertext);
    Ok(value)
}

fn decrypt(cipher: &XChaCha20Poly1305, value: &[u8]) -> Result<Vec<u8>, StoreError> {
    if value.len() < 24 {
        return Err(StoreError::Backend("stored secret is malformed".into()));
    }
    let (nonce, ciphertext) = value.split_at(24);
    cipher
        .decrypt(XNonce::from_slice(nonce), ciphertext)
        .map_err(|_| StoreError::Backend("failed to decrypt a stored secret".into()))
}

pub(super) fn seal_with(cipher: &XChaCha20Poly1305, plaintext: &str) -> Result<String, StoreError> {
    Ok(format!(
        "{PREFIX}{}",
        BASE64.encode(encrypt(cipher, plaintext.as_bytes())?)
    ))
}

/// Secrets always encrypt; an empty JSON map carries none, so it stays
/// plaintext and `enc:` values are already sealed.
fn seal_secret_with(cipher: &XChaCha20Poly1305, stored: &str) -> Result<String, StoreError> {
    if stored.starts_with(PREFIX) {
        Ok(stored.to_owned())
    } else {
        seal_with(cipher, stored)
    }
}

fn seal_map_with(cipher: &XChaCha20Poly1305, json: &str) -> Result<String, StoreError> {
    if json == "{}" {
        Ok(json.to_owned())
    } else {
        seal_secret_with(cipher, json)
    }
}

/// Sets up `app_key` (a single-row salt + canary table) on first keyed open
/// and verifies the passphrase against it on every open.
pub(super) fn setup(
    connection: &Connection,
    app_key: &AppKey,
) -> Result<XChaCha20Poly1305, StoreError> {
    let row = connection
        .query_row("SELECT salt, canary FROM app_key WHERE id = 1", [], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .optional()
        .map_err(backend)?;
    let (salt, canary) = match row {
        Some(row) => row,
        None => {
            let mut salt = [0u8; 16];
            OsRng.fill_bytes(&mut salt);
            let canary = encrypt(&cipher(&app_key.derive(&salt)?), CANARY)?;
            connection
                .execute(
                    "INSERT OR IGNORE INTO app_key (id, salt, canary) VALUES (1, ?1, ?2)",
                    params![salt.as_slice(), canary],
                )
                .map_err(backend)?;
            connection
                .query_row("SELECT salt, canary FROM app_key WHERE id = 1", [], |row| {
                    Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
                })
                .map_err(backend)?
        }
    };
    let cipher = cipher(&app_key.derive(&salt)?);
    match decrypt(&cipher, &canary) {
        Ok(canary) if canary == CANARY => Ok(cipher),
        _ => Err(StoreError::Backend(
            "BLOB_1_APP_KEY does not match this database".into(),
        )),
    }
}

/// Encrypts plaintext secrets left over from before the database was keyed.
/// Runs in one immediate transaction and only rewrites rows that still hold
/// plaintext, so it never overwrites a concurrent update. Freed plaintext is
/// zeroed, and `secret_scrub` stays set until a truncating checkpoint clears
/// the old pages out of the WAL; later opens retry it.
pub(super) fn migrate(
    connection: &Connection,
    cipher: &XChaCha20Poly1305,
) -> Result<(), StoreError> {
    connection
        .execute_batch("PRAGMA secure_delete = ON; BEGIN IMMEDIATE")
        .map_err(backend)?;
    let sealed = seal_plaintext(connection, cipher).and_then(|sealed| {
        if sealed > 0 {
            connection
                .execute("INSERT OR IGNORE INTO secret_scrub (id) VALUES (1)", [])
                .map_err(backend)?;
        }
        Ok(())
    });
    if let Err(error) = sealed {
        let _ = connection.execute_batch("ROLLBACK");
        return Err(error);
    }
    connection.execute_batch("COMMIT").map_err(backend)?;
    let pending = connection
        .query_row("SELECT 1 FROM secret_scrub", [], |_| Ok(()))
        .optional()
        .map_err(backend)?
        .is_some();
    if pending {
        let busy: i64 = connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
            .map_err(backend)?;
        if busy == 0 {
            connection
                .execute("DELETE FROM secret_scrub", [])
                .map_err(backend)?;
        }
    }
    Ok(())
}

fn seal_plaintext(
    connection: &Connection,
    cipher: &XChaCha20Poly1305,
) -> Result<usize, StoreError> {
    let mut sealed = 0;
    let servers = connection
        .prepare("SELECT id, env, headers FROM mcp_servers")
        .map_err(backend)?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(backend)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(backend)?;
    for (id, env, headers) in servers {
        let (new_env, new_headers) = (
            seal_map_with(cipher, &env)?,
            seal_map_with(cipher, &headers)?,
        );
        if new_env == env && new_headers == headers {
            continue;
        }
        sealed += 1;
        connection
            .execute(
                "UPDATE mcp_servers SET env = ?2, headers = ?3 WHERE id = ?1",
                params![id, new_env, new_headers],
            )
            .map_err(backend)?;
    }
    let tokens = connection
        .prepare("SELECT server, credentials, registration FROM mcp_tokens")
        .map_err(backend)?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(backend)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(backend)?;
    for (server, credentials, registration) in tokens {
        let seal = |value: &Option<String>| {
            value
                .as_deref()
                .map(|v| seal_secret_with(cipher, v))
                .transpose()
        };
        let (new_credentials, new_registration) = (seal(&credentials)?, seal(&registration)?);
        if new_credentials == credentials && new_registration == registration {
            continue;
        }
        sealed += 1;
        connection
            .execute(
                "UPDATE mcp_tokens SET credentials = ?2, registration = ?3 WHERE server = ?1",
                params![server, new_credentials, new_registration],
            )
            .map_err(backend)?;
    }
    let states = connection
        .prepare("SELECT csrf, state FROM mcp_oauth_states")
        .map_err(backend)?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(backend)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(backend)?;
    for (csrf, state) in states {
        if state.starts_with(PREFIX) {
            continue;
        }
        sealed += 1;
        connection
            .execute(
                "UPDATE mcp_oauth_states SET state = ?2 WHERE csrf = ?1",
                params![csrf, seal_secret_with(cipher, &state)?],
            )
            .map_err(backend)?;
    }
    let messengers = connection
        .prepare("SELECT messenger, token FROM messenger_tokens")
        .map_err(backend)?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(backend)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(backend)?;
    for (messenger, token) in messengers {
        if token.starts_with(PREFIX) {
            continue;
        }
        sealed += 1;
        connection
            .execute(
                "UPDATE messenger_tokens SET token = ?2 WHERE messenger = ?1",
                params![messenger, seal_secret_with(cipher, &token)?],
            )
            .map_err(backend)?;
    }
    Ok(sealed)
}

impl SqliteStorage {
    /// Fails with "set BLOB_1_APP_KEY" when the store was opened without a key.
    pub fn require_key(&self) -> Result<(), StoreError> {
        self.cipher().map(|_| ())
    }

    fn cipher(&self) -> Result<&XChaCha20Poly1305, StoreError> {
        self.key
            .as_ref()
            .ok_or_else(|| StoreError::Backend(NO_KEY.into()))
    }

    /// Encrypts a secret for storage; needs a key (`BLOB_1_APP_KEY` at open).
    pub(crate) fn seal(&self, plaintext: &str) -> Result<String, StoreError> {
        seal_with(self.cipher()?, plaintext)
    }

    /// Encrypts a serialized JSON map unless it is empty.
    pub(crate) fn seal_map(&self, json: &str) -> Result<String, StoreError> {
        if json == "{}" {
            Ok(json.to_owned())
        } else {
            self.seal(json)
        }
    }

    /// Decrypts a stored secret. Plaintext predating the key passes through on
    /// a keyed store (it is migrated at open); without a key it is an error.
    pub(crate) fn unseal(&self, stored: &str) -> Result<String, StoreError> {
        match stored.strip_prefix(PREFIX) {
            Some(data) => {
                let bytes = BASE64
                    .decode(data)
                    .map_err(|_| StoreError::Backend("stored secret is malformed".into()))?;
                String::from_utf8(decrypt(self.cipher()?, &bytes)?)
                    .map_err(|_| StoreError::Backend("stored secret is malformed".into()))
            }
            None if self.key.is_some() => Ok(stored.to_owned()),
            None => Err(StoreError::Backend(NO_KEY.into())),
        }
    }

    /// Decrypts a stored JSON map; "{}" is never encrypted.
    pub(crate) fn unseal_map(&self, stored: &str) -> Result<String, StoreError> {
        if stored == "{}" {
            Ok(stored.to_owned())
        } else {
            self.unseal(stored)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_unseal_and_wrong_key() {
        let salt = [7u8; 16];
        let right = cipher(&AppKey::new("correct".into()).derive(&salt).unwrap());
        let stored = seal_with(&right, "a secret").unwrap();
        assert!(stored.starts_with(PREFIX));
        let bytes = BASE64.decode(&stored[PREFIX.len()..]).unwrap();
        assert_eq!(decrypt(&right, &bytes).unwrap(), b"a secret");
        let wrong = cipher(&AppKey::new("wrong".into()).derive(&salt).unwrap());
        assert!(decrypt(&wrong, &bytes).is_err());
    }
}
