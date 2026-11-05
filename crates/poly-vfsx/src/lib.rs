//! Poly VFSX: signing and verification for packages
//!
//! This crate provides VFSX signing and verification for Poly packages.
//! VFSX is a signed archive format that ensures package integrity and authenticity.

pub mod signer;
pub mod verifier;
pub mod manifest;

pub use signer::VfsxSigner;
pub use verifier::VfsxVerifier;
pub use manifest::{VfsxManifest, PackageInfo, FileEntry, Signature};
