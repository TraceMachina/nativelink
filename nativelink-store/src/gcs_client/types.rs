// Copyright 2024 The NativeLink Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// ----- File size thresholds -----
/// Threshold for using simple upload vs. resumable upload (10MB)
pub const SIMPLE_UPLOAD_THRESHOLD: i64 = 10 * 1024 * 1024;
/// Minimum size for multipart upload (5MB)
pub const MIN_MULTIPART_SIZE: u64 = 5 * 1024 * 1024;
/// Default chunk size for uploads (~2MB)
pub const CHUNK_SIZE: usize = 2 * 1024 * 1000;

// ----- Upload retry configuration -----
/// Maximum number of upload retry attempts
pub const MAX_UPLOAD_RETRIES: u32 = 5;
/// Initial delay between upload retry attempts (500ms)
pub const INITIAL_UPLOAD_RETRY_DELAY_MS: u64 = 500;
/// Maximum delay between upload retry attempts (8s)
pub const MAX_UPLOAD_RETRY_DELAY_MS: u64 = 8000;

// ----- Connection pool configuration -----
/// Default number of concurrent uploads
pub const DEFAULT_CONCURRENT_UPLOADS: usize = 10;
/// Default buffer size for retrying requests (5MB)
pub const DEFAULT_MAX_RETRY_BUFFER_PER_REQUEST: usize = 5 * 1024 * 1024;

/// Default content type for uploaded objects
pub const DEFAULT_CONTENT_TYPE: &str = "application/octet-stream";

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
