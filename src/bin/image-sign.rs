//! Host packaging tool only. Never opens a destination disk.
#[path = "../image_manifest.rs"]
mod image_manifest;
use sha2::{Digest, Sha256};
use std::{fs, io::Read};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 6 {
        return Err("usage: installer-image-sign IMAGE MANIFEST RAW_PRIVATE_KEY VERSION BUILD EXPECTED_PUBLIC_KEY_HEX".into());
    }
    let secret = fs::read(&args[2])?;
    let key = ed25519_dalek::SigningKey::from_bytes(
        secret
            .as_slice()
            .try_into()
            .map_err(|_| "Expected a 32-byte raw signing key")?,
    );
    let public = key
        .verifying_key()
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if args[5].to_str() != Some(public.as_str()) {
        return Err("The private key does not match the expected public key".into());
    }
    let mut image = fs::File::open(&args[0])?;
    let metadata = image.metadata()?;
    if !metadata.is_file() {
        return Err("The source must be a regular image file".into());
    }
    let mut hash = Sha256::new();
    let mut b = vec![0; 1024 * 1024];
    loop {
        let n = image.read(&mut b)?;
        if n == 0 {
            break;
        }
        hash.update(&b[..n]);
    }
    let manifest = image_manifest::Manifest {
        version: args[3].to_str().ok_or("Invalid version")?.into(),
        build: args[4].to_str().ok_or("Invalid build")?.parse()?,
        bytes: metadata.len(),
        sha256: hash.finalize().into(),
        architecture: 1,
    };
    let bytes = manifest.sign(&key)?;
    let mut output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&args[1])?;
    use std::io::Write;
    output.write_all(&bytes)?;
    output.sync_all()?;
    Ok(())
}
