// Copyright 2021 Nathan (Blaise) Bruer.  All rights reserved.

use serde::Deserialize;

#[allow(non_camel_case_types)]
#[derive(Deserialize, Debug)]
pub enum StoreConfig {
    /// Memory store will store all data in a hashmap in memory.
    memory(MemoryStore),

    /// Verify store is used to apply verifications to an underlying
    /// store implementation. It is strongly encouraged to validate
    /// as much data as you can before accepting data from a client,
    /// failing to do so may cause the data in the store to be
    /// populated with invalid data causing all kinds of problems.
    ///
    /// The suggested configuration is to have the CAS validate the
    /// hash and size and the AC validate nothing.
    verify(Box<VerifyStore>),
}

#[derive(Deserialize, Debug)]
pub struct MemoryStore {
    // TODO(allada) MemoryStore needs to implement deleting deleting old objects when they are deleted.
}

#[derive(Deserialize, Debug)]
pub struct VerifyStore {
    #[serde(default)]
    /// The underlying store wrap around. All content will first flow
    /// through self before forwarding to backend. In the event there
    /// is an error detected in self, the connection to the backend
    /// will be terminated, and early termination should always cause
    /// updates to fail on the backend.
    pub backend: Option<StoreConfig>,

    /// If set the store will verify the size of the data before accepting
    /// an upload of data.
    ///
    /// This should be set to false for AC, but true for CAS stores.
    #[serde(default)]
    pub verify_size: bool,
}
