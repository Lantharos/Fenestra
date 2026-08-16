use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::Serialize;

use crate::{AppReleaseManifest, ServiceError, ServiceResult, SystemReleaseManifest};

pub fn public_key_from_private(encoded: &str) -> ServiceResult<String> {
    let seed = decode_array::<32>(encoded, "private signing key")?;
    Ok(STANDARD.encode(SigningKey::from_bytes(&seed).verifying_key().as_bytes()))
}

pub fn sign_app_release(
    manifest: &mut AppReleaseManifest,
    encoded_private_key: &str,
) -> ServiceResult<()> {
    manifest.signature.clear();
    manifest.signature = sign_value(manifest, encoded_private_key)?;
    Ok(())
}

pub fn verify_app_release(
    manifest: &AppReleaseManifest,
    encoded_public_key: &str,
) -> ServiceResult<()> {
    verify_value(manifest, &manifest.signature, encoded_public_key)
}

pub fn sign_system_release(
    manifest: &mut SystemReleaseManifest,
    encoded_private_key: &str,
) -> ServiceResult<()> {
    manifest.signature.clear();
    manifest.signature = sign_value(manifest, encoded_private_key)?;
    Ok(())
}

pub fn verify_system_release(
    manifest: &SystemReleaseManifest,
    encoded_public_key: &str,
) -> ServiceResult<()> {
    verify_value(manifest, &manifest.signature, encoded_public_key)
}

fn sign_value(value: &impl Serialize, encoded_private_key: &str) -> ServiceResult<String> {
    let seed = decode_array::<32>(encoded_private_key, "private signing key")?;
    let signing_key = SigningKey::from_bytes(&seed);
    let payload = serde_json::to_vec(value).map_err(|error| {
        ServiceError::Update(format!("could not encode signed metadata: {error}"))
    })?;
    Ok(STANDARD.encode(signing_key.sign(&payload).to_bytes()))
}

fn verify_value<T>(
    value: &T,
    encoded_signature: &str,
    encoded_public_key: &str,
) -> ServiceResult<()>
where
    T: Serialize + Clone + ClearSignature,
{
    if encoded_public_key.trim().is_empty() {
        return Err(ServiceError::Update(
            "update public key is missing".to_string(),
        ));
    }
    let public = decode_array::<32>(encoded_public_key, "update public key")?;
    let signature = decode_array::<64>(encoded_signature, "update signature")?;
    let verifying_key = VerifyingKey::from_bytes(&public)
        .map_err(|_| ServiceError::Update("update public key is invalid".to_string()))?;
    let mut unsigned = value.clone();
    unsigned.clear_signature();
    let payload = serde_json::to_vec(&unsigned).map_err(|error| {
        ServiceError::Update(format!("could not encode signed metadata: {error}"))
    })?;
    verifying_key
        .verify(&payload, &Signature::from_bytes(&signature))
        .map_err(|_| ServiceError::Update("update metadata signature is invalid".to_string()))
}

trait ClearSignature {
    fn clear_signature(&mut self);
}

impl ClearSignature for AppReleaseManifest {
    fn clear_signature(&mut self) {
        self.signature.clear();
    }
}

impl ClearSignature for SystemReleaseManifest {
    fn clear_signature(&mut self) {
        self.signature.clear();
    }
}

fn decode_array<const N: usize>(encoded: &str, label: &str) -> ServiceResult<[u8; N]> {
    let bytes = STANDARD
        .decode(encoded.trim())
        .map_err(|_| ServiceError::Update(format!("{label} is not valid base64")))?;
    bytes
        .try_into()
        .map_err(|_| ServiceError::Update(format!("{label} must contain {N} bytes")))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::AppReleaseManifest;

    fn private_key() -> String {
        STANDARD.encode([7_u8; 32])
    }

    #[test]
    fn signed_app_metadata_rejects_tampering() {
        let private = private_key();
        let public = public_key_from_private(&private).unwrap();
        let mut manifest = AppReleaseManifest {
            schema: 1,
            app_id: "com.example.app".to_string(),
            version: "1.0.0".to_string(),
            channel: "stable".to_string(),
            artifacts: BTreeMap::new(),
            signature: String::new(),
        };
        sign_app_release(&mut manifest, &private).unwrap();
        verify_app_release(&manifest, &public).unwrap();
        manifest.version = "2.0.0".to_string();
        assert!(verify_app_release(&manifest, &public).is_err());
    }

    #[test]
    fn signed_system_metadata_rejects_tampering() {
        let private_key = STANDARD.encode([19_u8; 32]);
        let public_key = public_key_from_private(&private_key).unwrap();
        let mut manifest = SystemReleaseManifest {
            schema: 1,
            version: "1.2.3".to_string(),
            published_at: "12345".to_string(),
            artifacts: BTreeMap::from([(
                "sabine-system-windows-x86_64.zip".to_string(),
                crate::SystemReleaseArtifact {
                    sha256: "a".repeat(64),
                    size: 42,
                    url: "https://example.invalid/system.zip".to_string(),
                },
            )]),
            signature: String::new(),
        };
        sign_system_release(&mut manifest, &private_key).unwrap();
        verify_system_release(&manifest, &public_key).unwrap();
        manifest.artifacts.values_mut().next().unwrap().size += 1;
        assert!(verify_system_release(&manifest, &public_key).is_err());
    }
}
