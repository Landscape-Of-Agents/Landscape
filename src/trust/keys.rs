// Repository signing keys at rest. A key file holds either the legacy plain seed (64 hex
// characters) or a sealed key: JSON with the public key in the clear (so hooks and verification
// never unseal anything) and the seed sealed by
//   - dpapi: the Windows Data Protection API, tied to the user's Windows login (the default on
//     Windows: a copied file, a backup, or another account cannot use it);
//   - passphrase: Argon2id and XChaCha20-Poly1305 under a passphrase typed on the console for each
//     signing command (CRANE_KEY_PROTECTION=prompt, on any system). Unlike the other modes it
//     also stops a process running as the same user, such as an agent's shell, since nothing can
//     answer the console prompt for it.
// Elsewhere the default stays the plain seed with owner-only permissions. A plain key is sealed
// the next time a human command loads it; a key is never unsealed by the hook runtime.

use serde::{Deserialize, Serialize};

use super::crypto::{hex, random_bytes, unhex, SigningIdentity};

/** Layout version of sealed key files */
const FORMAT: &str = "crane-key/v1";

/** How a signing key is kept
 * Variants
    - Plain - the seed in hex (owner-only permissions)
    - Dpapi - sealed with the Windows Data Protection API
    - Passphrase - sealed under a passphrase typed on the console
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Protection {
    Plain,
    Dpapi,
    Passphrase,
}

impl Protection {
    /** Return the protection's name in key files
     * Input
        - None (uses self)
     * Output
        - &'static str
    */
    fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Dpapi => "dpapi",
            Self::Passphrase => "passphrase",
        }
    }
}

/** A sealed key file
 * Fields
    - format: String - FORMAT
    - public: String - public key in hex
    - protection: String - dpapi or passphrase
    - sealed: String - the sealed seed in hex
    - salt: String - Argon2id salt in hex (passphrase)
    - nonce: String - XChaCha20-Poly1305 nonce in hex (passphrase)
*/
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Sealed {
    format: String,
    public: String,
    protection: String,
    sealed: String,
    #[serde(default)]
    salt: String,
    #[serde(default)]
    nonce: String,
}

/** Return the protection new and migrated keys get: CRANE_KEY_PROTECTION (prompt, plain, or
 * os), by default the operating system's protection where Crane has one (Windows), else plain
 * Input
    - None
 * Output
    - Protection
*/
pub(crate) fn wanted() -> Protection {
    match std::env::var("CRANE_KEY_PROTECTION")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "prompt" | "passphrase" => Protection::Passphrase,
        "plain" => Protection::Plain,
        _ if cfg!(windows) => Protection::Dpapi,
        _ => Protection::Plain,
    }
}

/** Return how a key file's content is protected
 * Input
    - text: &str - key file content
 * Output
    - Protection (plain for the legacy seed or anything unrecognized)
*/
pub(crate) fn protection(text: &str) -> Protection {
    match serde_json::from_str::<Sealed>(text.trim()).map(|sealed| sealed.protection) {
        Ok(name) if name == "dpapi" => Protection::Dpapi,
        Ok(name) if name == "passphrase" => Protection::Passphrase,
        _ => Protection::Plain,
    }
}

/** Return a key file's public key without unsealing it
 * Input
    - text: &str - key file content
 * Output
    - Result<String, String> public key in hex
*/
pub(crate) fn public_key(text: &str) -> Result<String, String> {
    if text.trim_start().starts_with('{') {
        let sealed: Sealed = serde_json::from_str(text.trim())
            .map_err(|error| format!("malformed key file ({error})"))?;
        return Ok(sealed.public);
    }
    SigningIdentity::from_seed_hex(text).map(|identity| identity.public_hex())
}

/** Unseal a key file (only human commands do this: it may ask for the passphrase)
 * Input
    - text: &str - key file content
 * Output
    - Result<SigningIdentity, String>
    - Error if it cannot be unsealed here (another Windows user, a wrong passphrase, no console)
*/
pub(crate) fn open(text: &str) -> Result<SigningIdentity, String> {
    if !text.trim_start().starts_with('{') {
        return SigningIdentity::from_seed_hex(text);
    }
    let sealed: Sealed = serde_json::from_str(text.trim())
        .map_err(|error| format!("malformed key file ({error})"))?;
    if sealed.format != FORMAT {
        return Err(format!("unsupported key file format {}", sealed.format));
    }
    let seed = match sealed.protection.as_str() {
        "dpapi" => dpapi::unprotect(&unhex(&sealed.sealed)?)?,
        "passphrase" => {
            let passphrase = passphrase("Crane signing key passphrase: ", false)?;
            unseal_with(&sealed, &passphrase)?
        }
        other => return Err(format!("unknown key protection '{other}'")),
    };
    let identity = SigningIdentity::from_seed_hex(&String::from_utf8_lossy(&seed))?;
    if identity.public_hex() != sealed.public {
        return Err("the unsealed key does not match its public key".into());
    }
    Ok(identity)
}

/** Seal a key for its file
 * Input
    - identity: &SigningIdentity - key
    - protection: Protection - how to keep it
 * Output
    - Result<String, String> file content
*/
pub(crate) fn seal(identity: &SigningIdentity, protection: Protection) -> Result<String, String> {
    let seed = identity.seed_hex();
    let sealed = match protection {
        Protection::Plain => return Ok(format!("{seed}\n")),
        Protection::Dpapi => Sealed {
            format: FORMAT.into(),
            public: identity.public_hex(),
            protection: protection.name().into(),
            sealed: hex(&dpapi::protect(seed.as_bytes())?),
            salt: String::new(),
            nonce: String::new(),
        },
        Protection::Passphrase => {
            let passphrase = passphrase("New passphrase for the Crane signing key: ", true)?;
            seal_with(identity, &passphrase)?
        }
    };
    serde_json::to_string_pretty(&sealed)
        .map(|text| text + "\n")
        .map_err(|error| error.to_string())
}

/** Derive the passphrase key with Argon2id
 * Input
    - passphrase: &str - passphrase
    - salt: &[u8] - salt
 * Output
    - Result<[u8; 32], String>
*/
fn derive(passphrase: &str, salt: &[u8]) -> Result<[u8; 32], String> {
    let mut key = [0u8; 32];
    argon2::Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|error| format!("could not derive the key: {error}"))?;
    Ok(key)
}

/** Seal a key under a passphrase
 * Input
    - identity: &SigningIdentity - key
    - passphrase: &str - passphrase
 * Output
    - Result<Sealed, String>
*/
fn seal_with(identity: &SigningIdentity, passphrase: &str) -> Result<Sealed, String> {
    use chacha20poly1305::aead::{Aead, KeyInit};
    let salt = random_bytes(16)?;
    let nonce = random_bytes(24)?;
    let cipher = chacha20poly1305::XChaCha20Poly1305::new(&derive(passphrase, &salt)?.into());
    let sealed = cipher
        .encrypt(
            chacha20poly1305::XNonce::from_slice(&nonce),
            identity.seed_hex().as_bytes(),
        )
        .map_err(|_| "could not seal the key".to_string())?;
    Ok(Sealed {
        format: FORMAT.into(),
        public: identity.public_hex(),
        protection: Protection::Passphrase.name().into(),
        sealed: hex(&sealed),
        salt: hex(&salt),
        nonce: hex(&nonce),
    })
}

/** Unseal a passphrase-sealed key
 * Input
    - sealed: &Sealed - sealed key
    - passphrase: &str - passphrase
 * Output
    - Result<Vec<u8>, String> the seed in hex
*/
fn unseal_with(sealed: &Sealed, passphrase: &str) -> Result<Vec<u8>, String> {
    use chacha20poly1305::aead::{Aead, KeyInit};
    let cipher = chacha20poly1305::XChaCha20Poly1305::new(
        &derive(passphrase, &unhex(&sealed.salt)?)?.into(),
    );
    let nonce = unhex(&sealed.nonce)?;
    if nonce.len() != 24 {
        return Err("malformed key file (nonce)".into());
    }
    cipher
        .decrypt(
            chacha20poly1305::XNonce::from_slice(&nonce),
            unhex(&sealed.sealed)?.as_slice(),
        )
        .map_err(|_| "wrong passphrase for the Crane signing key".to_string())
}

/** Ask for the passphrase on the console (never from piped input, so a program cannot answer)
 * Input
    - prompt: &str - prompt
    - confirm: bool - ask twice (new passphrases)
 * Output
    - Result<String, String>
*/
fn passphrase(prompt: &str, confirm: bool) -> Result<String, String> {
    // Debug builds only: lets the test suite exercise passphrase keys without a console
    if cfg!(debug_assertions) {
        if let Ok(value) = std::env::var("CRANE_TEST_KEY_PASSPHRASE") {
            return Ok(value);
        }
    }
    let read = |prompt: &str| {
        rpassword::prompt_password(prompt).map_err(|error| {
            format!("the signing key passphrase must be typed on a console ({error})")
        })
    };
    let first = read(prompt)?;
    if first.chars().count() < 8 {
        return Err("use a passphrase of at least 8 characters".into());
    }
    if confirm && read("Repeat the passphrase: ")? != first {
        return Err("the passphrases differ; nothing changed".into());
    }
    Ok(first)
}

/** The Windows Data Protection API, scoped to the current user */
#[cfg(windows)]
pub(super) mod dpapi {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};

    /** A byte buffer passed to and returned by the API (CRYPT_INTEGER_BLOB) */
    #[repr(C)]
    struct Blob {
        size: u32,
        data: *mut u8,
    }

    /** Never show a prompt (CRYPTPROTECT_UI_FORBIDDEN) */
    const UI_FORBIDDEN: u32 = 0x1;

    #[link(name = "crypt32")]
    extern "system" {
        /** Seal data for the current user (Win32) */
        fn CryptProtectData(
            input: *const Blob,
            description: *const u16,
            entropy: *const Blob,
            reserved: *const c_void,
            prompt: *const c_void,
            flags: u32,
            output: *mut Blob,
        ) -> i32;
        /** Unseal data sealed for the current user (Win32) */
        fn CryptUnprotectData(
            input: *const Blob,
            description: *mut *mut u16,
            entropy: *const Blob,
            reserved: *const c_void,
            prompt: *const c_void,
            flags: u32,
            output: *mut Blob,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        /** Free memory the API allocated (Win32) */
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }

    /** Run one of the two calls and copy out its result
     * Input
        - data: &[u8] - input
        - seal: bool - CryptProtectData (true) or CryptUnprotectData (false)
     * Output
        - Result<Vec<u8>, String>
    */
    fn call(data: &[u8], seal: bool) -> Result<Vec<u8>, String> {
        let input = Blob {
            size: u32::try_from(data.len()).map_err(|_| "the key is too large".to_string())?,
            data: data.as_ptr().cast_mut(),
        };
        let mut output = Blob {
            size: 0,
            data: null_mut(),
        };
        // SAFETY: input points at data, which outlives the call and is only read; the optional
        // arguments are null, which the API allows; on success output points at a buffer the API
        // allocated with LocalAlloc, which is copied and then freed exactly once below
        let ok = unsafe {
            if seal {
                CryptProtectData(
                    &input,
                    null(),
                    null(),
                    null(),
                    null(),
                    UI_FORBIDDEN,
                    &mut output,
                )
            } else {
                CryptUnprotectData(
                    &input,
                    null_mut(),
                    null(),
                    null(),
                    null(),
                    UI_FORBIDDEN,
                    &mut output,
                )
            }
        };
        if ok == 0 || output.data.is_null() {
            return Err(if seal {
                "Windows could not protect the signing key".into()
            } else {
                "Windows could not unprotect the signing key (it was sealed by another Windows user or machine)".into()
            });
        }
        // SAFETY: the API returned output.size readable bytes at output.data
        let bytes =
            unsafe { std::slice::from_raw_parts(output.data, output.size as usize) }.to_vec();
        // SAFETY: output.data was allocated by the API and is not used after this
        unsafe {
            LocalFree(output.data.cast());
        }
        Ok(bytes)
    }

    /** Seal data for the current Windows user
     * Input
        - data: &[u8] - data
     * Output
        - Result<Vec<u8>, String>
    */
    pub(crate) fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
        call(data, true)
    }

    /** Unseal data sealed for the current Windows user
     * Input
        - data: &[u8] - sealed data
     * Output
        - Result<Vec<u8>, String>
    */
    pub(crate) fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
        call(data, false)
    }
}

/** Without the Windows Data Protection API, sealing with it is refused */
#[cfg(not(windows))]
pub(super) mod dpapi {
    /** Refuse: not available here
     * Input
        - _data: &[u8] - data
     * Output
        - Result<Vec<u8>, String> always an error
    */
    pub(crate) fn protect(_data: &[u8]) -> Result<Vec<u8>, String> {
        Err("the Windows Data Protection API is only available on Windows".into())
    }

    /** Refuse: not available here
     * Input
        - _data: &[u8] - data
     * Output
        - Result<Vec<u8>, String> always an error
    */
    pub(crate) fn unprotect(_data: &[u8]) -> Result<Vec<u8>, String> {
        Err("this key was sealed on Windows and can only be used there".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /** Check that keys round-trip through every protection available here, that the public key
     * is readable without unsealing, and that a wrong passphrase fails
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn keys_round_trip() {
        let identity = SigningIdentity::generate().unwrap();
        let plain = seal(&identity, Protection::Plain).unwrap();
        assert_eq!(protection(&plain), Protection::Plain);
        assert_eq!(open(&plain).unwrap().public_hex(), identity.public_hex());
        let sealed =
            serde_json::to_string(&seal_with(&identity, "correct horse").unwrap()).unwrap();
        assert_eq!(protection(&sealed), Protection::Passphrase);
        assert_eq!(public_key(&sealed).unwrap(), identity.public_hex());
        assert!(!sealed.contains(&identity.seed_hex()));
        let parsed: Sealed = serde_json::from_str(&sealed).unwrap();
        assert!(unseal_with(&parsed, "wrong horse").is_err());
        assert_eq!(
            unseal_with(&parsed, "correct horse").unwrap(),
            identity.seed_hex().into_bytes()
        );
        if cfg!(windows) {
            let protected = seal(&identity, Protection::Dpapi).unwrap();
            assert_eq!(protection(&protected), Protection::Dpapi);
            assert!(!protected.contains(&identity.seed_hex()));
            assert_eq!(public_key(&protected).unwrap(), identity.public_hex());
            assert_eq!(
                open(&protected).unwrap().public_hex(),
                identity.public_hex()
            );
        }
    }
}
