// Copyright (c) The Diem Core Contributors
// SPDX-License-Identifier: Apache-2.0
use crate::{
    CryptoStorage, Error, GetResponse, GitHubStorage, InMemoryStorage,
    KVStorage, NamespacedStorage, OnDiskStorage, PublicKeyResponse,
    VaultStorage,
};
use diem_crypto::PrivateKey;
use diem_types::validator_config::{
    ConsensusPrivateKey, ConsensusPublicKey, ConsensusSignature,
};
use enum_dispatch::enum_dispatch;
use serde::{de::DeserializeOwned, Serialize};

/// This is the Diem interface into secure storage. Any storage engine
/// implementing this trait should support both key/value operations (e.g., get,
/// set and create) and cryptographic key operations (e.g., generate_key, sign
/// and rotate_key).

/// This is a hack that allows us to convert from SecureBackend into a useable
/// T: Storage. This boilerplate can be 100% generated by a proc macro.
#[enum_dispatch(KVStorage, CryptoStorage)]
pub enum Storage {
    GitHubStorage(GitHubStorage),
    VaultStorage(VaultStorage),
    InMemoryStorage(InMemoryStorage),
    NamespacedStorage(NamespacedStorage),
    OnDiskStorage(OnDiskStorage),
}
