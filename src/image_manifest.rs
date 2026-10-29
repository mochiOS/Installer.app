//! Separate signature domain for a complete, clean installation template.
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
pub const LENGTH: usize = 256;
const CONTEXT: &[u8] = b"mochios-install-image-v1\0";
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub version: String,
    pub build: u64,
    pub bytes: u64,
    pub sha256: [u8; 32],
    pub architecture: u16,
}
fn message(bytes: &[u8]) -> Vec<u8> {
    [CONTEXT, &bytes[..192]].concat()
}
impl Manifest {
    pub fn sign(&self, key: &SigningKey) -> Result<[u8; LENGTH], String> {
        if self.version.is_empty()
            || self.version.len() > 63
            || !self
                .version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
            || self.build == 0
            || self.bytes < 512 * 68
            || self.bytes % 512 != 0
            || self.architecture != 1
        {
            return Err("Invalid installation manifest.".into());
        }
        let mut b = [0; LENGTH];
        b[..8].copy_from_slice(b"MOINS1\0\0");
        b[8..10].copy_from_slice(&self.architecture.to_le_bytes());
        b[10..12].copy_from_slice(&(self.version.len() as u16).to_le_bytes());
        b[16..24].copy_from_slice(&self.bytes.to_le_bytes());
        b[24..32].copy_from_slice(&self.build.to_le_bytes());
        b[32..64].copy_from_slice(&self.sha256);
        b[64..64 + self.version.len()].copy_from_slice(self.version.as_bytes());
        b[128..160].copy_from_slice(&Sha256::digest(key.verifying_key().as_bytes()));
        let signature = key.sign(&message(&b));
        b[192..].copy_from_slice(&signature.to_bytes());
        Ok(b)
    }
    pub fn verify(b: &[u8], keys: &[[u8; 32]]) -> Result<Self, String> {
        if b.len() != LENGTH
            || &b[..8] != b"MOINS1\0\0"
            || b[12..16] != [0; 4]
            || b[160..192] != [0; 32]
        {
            return Err("Invalid installation manifest.".into());
        }
        let n = u16::from_le_bytes(b[10..12].try_into().unwrap()) as usize;
        if !(1..=63).contains(&n) || b[64 + n..128].iter().any(|v| *v != 0) {
            return Err("Invalid installation version.".into());
        }
        let key = keys
            .iter()
            .find(|k| Sha256::digest(k).as_slice() == &b[128..160])
            .ok_or("Untrusted installation signing key.")?;
        VerifyingKey::from_bytes(key)
            .map_err(|_| "Invalid signing key.")?
            .verify_strict(
                &message(b),
                &Signature::from_bytes(b[192..].try_into().unwrap()),
            )
            .map_err(|_| "Installation signature verification failed.")?;
        let m = Self {
            version: std::str::from_utf8(&b[64..64 + n])
                .map_err(|_| "Invalid version.")?
                .into(),
            build: u64::from_le_bytes(b[24..32].try_into().unwrap()),
            bytes: u64::from_le_bytes(b[16..24].try_into().unwrap()),
            sha256: b[32..64].try_into().unwrap(),
            architecture: u16::from_le_bytes(b[8..10].try_into().unwrap()),
        };
        if m.architecture != 1
            || m.bytes < 512 * 68
            || m.bytes % 512 != 0
            || m.build == 0
            || !m
                .version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
        {
            return Err("Unsupported installation manifest.".into());
        }
        Ok(m)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signatures_bind_all_template_metadata_and_deny_unknown_keys() {
        let key = SigningKey::from_bytes(&[9; 32]);
        let public = key.verifying_key().to_bytes();
        let m = Manifest {
            version: "26.0.0".into(),
            build: 42,
            bytes: 1024 * 1024,
            sha256: [1; 32],
            architecture: 1,
        };
        let bytes = m.sign(&key).unwrap();
        assert_eq!(Manifest::verify(&bytes, &[public]).unwrap(), m);
        for offset in [0, 8, 10, 16, 24, 32, 64, 128, 160, 192, 255] {
            let mut bad = bytes;
            bad[offset] ^= 1;
            assert!(Manifest::verify(&bad, &[public]).is_err());
        }
        assert!(Manifest::verify(&bytes, &[[0; 32]]).is_err());
        assert!(Manifest::verify(&bytes[..255], &[public]).is_err());
    }
}
