// Copyright 2022 The Turbo Cache Authors. All rights reserved.
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

use error::{error_if, Error, ResultExt};

// Useful utility struct for converting bazel's (uri-like path) into it's parts.
pub struct ResourceInfo<'a> {
    pub instance_name: &'a str,
    pub uuid: Option<&'a str>,
    pub hash: &'a str,
    pub expected_size: usize,
}

impl<'a> ResourceInfo<'a> {
    pub fn new(resource_name: &'a str) -> Result<ResourceInfo<'a>, Error> {
        let mut parts = resource_name.splitn(6, '/').peekable();
        const ERROR_MSG: &str = concat!(
            "Expected resource_name to be of pattern ",
            "'{?instance_name/}uploads/{uuid}/blobs/{hash}/{size}' or ",
            "'{?instance_name/}blobs/{hash}/{size}'",
        );

        let mut instance_name = "";
        let maybe_instance_name = parts.next().err_tip(|| ERROR_MSG)?;
        let peek_next = parts.peek().err_tip(|| ERROR_MSG)?;
        let blobs_or_uploads = if *peek_next == "uploads" || *peek_next == "blobs" {
            instance_name = maybe_instance_name;
            // We do have an instance_name, so set blobs_or_uploads to the peek_next and consume it.
            parts.next().err_tip(|| ERROR_MSG)?
        } else {
            // We don't have an instance_name, so use our first item as blobs_or_uploads.
            maybe_instance_name
        };
        let mut uuid = None;
        if blobs_or_uploads == "uploads" {
            uuid = Some(parts.next().err_tip(|| ERROR_MSG)?);
            let blobs = parts.next().err_tip(|| ERROR_MSG)?;
            error_if!(
                blobs != "blobs",
                "Expected resource_name to have 'blobs' here. Got: {}",
                blobs_or_uploads
            );
        }

        let hash = parts.next().err_tip(|| ERROR_MSG)?;
        let raw_digest_size = parts.next().err_tip(|| ERROR_MSG)?;
        let expected_size = raw_digest_size.parse::<usize>().err_tip(|| {
            format!(
                "Digest size_bytes was not convertible to usize. Got: {}",
                raw_digest_size
            )
        })?;

        Ok(ResourceInfo {
            instance_name,
            uuid,
            hash,
            expected_size,
        })
    }
}
