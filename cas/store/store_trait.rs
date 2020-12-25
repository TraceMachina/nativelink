// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::fmt::Debug;

pub trait StoreTrait: Sync + Send + Debug {
    fn has(&self, hash: &str) -> bool;
}
