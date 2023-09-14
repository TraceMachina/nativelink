// Copyright 2023 The Turbo Cache Authors. All rights reserved.
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

use error::{make_input_err, Error, ResultExt};

// Useful utility struct for converting bazel's (uri-like path) into it's parts.
pub struct ResourceInfo<'a> {
    pub instance_name: &'a str,
    pub uuid: Option<&'a str>,
    pub digest_function: Option<&'a str>,
    pub compressor: Option<&'a str>,
    pub hash: &'a str,
    pub expected_size: usize,
    pub optional_metadata: Option<&'a str>,
}

impl<'a> ResourceInfo<'a> {
    pub fn new(resource_name: &'a str) -> Result<ResourceInfo<'a>, Error> {
        let parts: Vec<&str> = resource_name.split('/').collect();
        const ERROR_MSG: &str = concat!(
            "Expected resource_name to be of pattern ",
            "'{?instance_name/}uploads/{uuid}/blobs/{digest_function/}{hash}/{size}{/optional_metadata}' or ",
            "'{?instance_name/}uploads/{uuid}/compressed-blobs/{compressor}/{digest_function/}{hash}/{size}{/optional_metadata}' or ",
            "'{?instance_name/}blobs/{hash}/{size}'",
        );
        const DIGEST_FUNCTIONS: [&str; 9] = [
            "sha256",
            "sha1",
            "md5",
            "vso",
            "sha384",
            "sha512",
            "murmur3",
            "sha256tree",
            "blake3",
        ];

        // The instance_name may contain '/' so parsing forwards isn't possible
        // without matching against our instance names.  We look for the first
        // instance of 'blobs' or 'compressed-blobs' at least two parts from
        // the end (to avoid it being in optional_metadata) and then work
        // backwards from there to get the uuid and instance_name and forwards
        // from there to get the hash, size, digest_function and
        // optional_metadata.
        let Some(blobs_position) = parts.iter().position(|part| *part == "blobs" || *part == "compressed-blobs") else {
            return Err(make_input_err!("{}", ERROR_MSG));
        };
        if parts.len() >= 3 && blobs_position > parts.len() - 3 {
            return Err(make_input_err!("{}", ERROR_MSG));
        }

        // Check to see if we have the format uploads/{uuid}.
        let uuid = if blobs_position >= 2 && parts.get(blobs_position - 2) == Some(&"uploads") {
            parts.get(blobs_position - 1).copied()
        } else {
            None
        };

        // The length of the instance_name is the length of all parts before
        // the blobs_position if uuid is None, otherwise it's all parts before
        // the blobs_position - 2.
        let mut instance_name_length = match uuid {
            Some(_) => parts[..blobs_position - 2].iter(),
            None => parts[..blobs_position].iter(),
        }
        .fold(0, |sum, part| sum + part.len() + 1);
        // Remove the final slash if the name is specified.
        if instance_name_length > 1 {
            instance_name_length -= 1;
        }
        let instance_name = &resource_name[..instance_name_length];

        // Now we are iterating forwards, so simplify things with a simple
        // iterator.
        let mut parts = parts[blobs_position..].iter().peekable();

        // Get the compressor name if this is a compressed-blobs request.
        let compressor = if *parts.next().err_tip(|| ERROR_MSG)? == "compressed-blobs" {
            Some(*parts.next().err_tip(|| ERROR_MSG)?)
        } else {
            None
        };

        // See if there is a digest function specified.
        let digest_function = if DIGEST_FUNCTIONS.contains(parts.peek().err_tip(|| ERROR_MSG)?) {
            Some(*parts.next().unwrap())
        } else {
            None
        };

        // From this point on it's a pretty simple hash, size and optional
        // metadata.
        let hash = *parts.next().err_tip(|| ERROR_MSG)?;
        let raw_digest_size = *parts.next().err_tip(|| ERROR_MSG)?;
        let expected_size = raw_digest_size.parse::<usize>().err_tip(|| {
            format!(
                "Digest size_bytes was not convertible to usize. Got: {}",
                raw_digest_size
            )
        })?;

        // Optional metadata might have slashes also, so get the length and
        // create a slice like we did with the instance_name.
        let metadata_length = parts.fold(0, |sum, part| sum + part.len() + 1);
        let optional_metadata = if metadata_length > 0 {
            Some(&resource_name[resource_name.len() - metadata_length + 1..])
        } else {
            None
        };

        Ok(ResourceInfo {
            instance_name,
            uuid,
            digest_function,
            compressor,
            hash,
            expected_size,
            optional_metadata,
        })
    }
}
