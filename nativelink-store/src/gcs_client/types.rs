// Copyright 2024 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

/// Default chunk size for uploads (~2MB)
pub const CHUNK_SIZE: usize = 2 * 1024 * 1000;

// ----- Connection pool configuration -----
/// Default number of concurrent uploads
pub const DEFAULT_CONCURRENT_UPLOADS: usize = 10;

#[derive(Debug, Clone)]
pub struct GcsObject {
    pub name: String,
    pub bucket: String,
    pub size: i64,
    pub content_type: String,
    pub update_time: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy)]
pub struct Timestamp {
    pub seconds: i64,
    pub nanos: i32,
}

#[derive(Clone, Debug)]
pub struct ObjectPath {
    pub bucket: String,
    pub path: String,
}

impl ObjectPath {
    pub fn new(bucket: String, path: &str) -> Self {
        let normalized_path = path.replace('\\', "/").trim_start_matches('/').to_string();
        Self {
            bucket,
            path: normalized_path,
        }
    }

    pub fn get_formatted_bucket(&self) -> String {
        format!("projects/_/buckets/{}", self.bucket)
    }
}
