// Copyright 2024 Trace Machina, Inc. All rights reserved.
//
// Licensed under the Business Source License, Version 1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may requested a copy of the License by emailing contact@nativelink.com.
//
// Use of this module requires an enterprise license agreement, which can be
// attained by emailing contact@nativelink.com or signing up for Nativelink
// Cloud at app.nativelink.com.
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Wire-format types for Bazel's persistent worker protocol.
//!
//! Reference: <https://bazel.build/remote/persistent#work-protocol>
//! and <https://github.com/bazelbuild/bazel/blob/master/src/main/protobuf/worker_protocol.proto>.
//!
//! Both JSON (newline-delimited, camelCase field names) and proto (length-delimited
//! varint prefix) wire formats are supported. The choice is per-request, driven by
//! the action's `requires-worker-protocol` execution requirement.

use bytes::{Bytes, BytesMut};
use nativelink_error::{Code, Error, make_err};
use prost::Message as ProstMessage;
use serde::{Deserialize, Serialize};

/// One input file declared in a `WorkRequest`. The tool may use the digest to
/// verify cached state matches.
#[derive(Clone, PartialEq, Eq, ProstMessage, Serialize, Deserialize)]
pub struct Input {
    #[prost(string, tag = "1")]
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,

    #[prost(bytes = "vec", tag = "2")]
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        with = "serde_bytes_b64"
    )]
    pub digest: Vec<u8>,
}

/// A single unit of work dispatched to a persistent worker subprocess.
#[derive(Clone, PartialEq, Eq, ProstMessage, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkRequest {
    #[prost(string, repeated, tag = "1")]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<String>,

    #[prost(message, repeated, tag = "2")]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<Input>,

    /// Multiplex worker request id. v1 always sends 0 and rejects responses
    /// with non-zero ids.
    #[prost(int32, tag = "3")]
    #[serde(default)]
    pub request_id: i32,

    #[prost(bool, tag = "4")]
    #[serde(default)]
    pub cancel: bool,

    #[prost(int32, tag = "5")]
    #[serde(default)]
    pub verbosity: i32,

    #[prost(string, tag = "6")]
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sandbox_dir: String,
}

/// A single response emitted by a persistent worker subprocess.
#[derive(Clone, PartialEq, Eq, ProstMessage, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkResponse {
    #[prost(int32, tag = "1")]
    #[serde(default)]
    pub exit_code: i32,

    #[prost(string, tag = "2")]
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,

    /// Echoed from `WorkRequest.request_id`. v1 expects 0.
    #[prost(int32, tag = "3")]
    #[serde(default)]
    pub request_id: i32,

    #[prost(bool, tag = "4")]
    #[serde(default)]
    pub was_cancelled: bool,
}

/// Wire format negotiated per request from the action's
/// `requires-worker-protocol` execution requirement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WireFormat {
    /// Length-delimited proto (Bazel default).
    Proto,
    /// Newline-delimited JSON, camelCase fields.
    Json,
}

impl WireFormat {
    /// Parse the execution-requirement value. Bazel accepts only "proto" and
    /// "json"; anything else is an error.
    pub fn parse(s: &str) -> Result<Self, Error> {
        match s {
            "proto" => Ok(Self::Proto),
            "json" => Ok(Self::Json),
            other => Err(make_err!(
                Code::InvalidArgument,
                "Unsupported requires-worker-protocol value: {other:?}; expected 'proto' or 'json'"
            )),
        }
    }
}

impl WorkRequest {
    /// Serialize this request in the given wire format. The returned bytes
    /// include the framing prefix/suffix appropriate for the format
    /// (length-delimited varint for proto, trailing newline for JSON).
    pub fn encode_framed(&self, format: WireFormat) -> Result<Bytes, Error> {
        match format {
            WireFormat::Proto => {
                let mut buf =
                    BytesMut::with_capacity(<Self as ProstMessage>::encoded_len(self) + 10);
                <Self as ProstMessage>::encode_length_delimited(self, &mut buf)
                    .map_err(|e| make_err!(Code::Internal, "WorkRequest proto encode: {e}"))?;
                Ok(buf.freeze())
            }
            WireFormat::Json => {
                let mut s = serde_json::to_string(self)
                    .map_err(|e| make_err!(Code::Internal, "WorkRequest JSON encode: {e}"))?;
                s.push('\n');
                Ok(Bytes::from(s))
            }
        }
    }
}

impl WorkResponse {
    /// Decode a single response from a byte buffer. For proto: expects a
    /// length-delimited varint frame at the buffer start. For JSON: expects a
    /// single complete JSON object, optionally with a trailing newline.
    pub fn decode_framed(buf: &[u8], format: WireFormat) -> Result<Self, Error> {
        match format {
            WireFormat::Proto => <Self as ProstMessage>::decode_length_delimited(buf)
                .map_err(|e| make_err!(Code::Internal, "WorkResponse proto decode: {e}")),
            WireFormat::Json => serde_json::from_slice(buf)
                .map_err(|e| make_err!(Code::Internal, "WorkResponse JSON decode: {e}")),
        }
    }
}

/// Serde adapter for `Input.digest`: Bazel's JSON serialization base64-encodes
/// the digest bytes (as is standard for proto-JSON byte fields).
mod serde_bytes_b64 {
    use serde::{Deserialize, Deserializer, Serializer};

    // Minimal RFC 4648 base64 encoder/decoder so we don't pull in a `base64`
    // crate dep just for this adapter. v1 expects digests under ~64 bytes.
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    fn encode(input: &[u8]) -> String {
        let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
        for chunk in input.chunks(3) {
            let b0 = chunk[0];
            let b1 = chunk.get(1).copied().unwrap_or(0);
            let b2 = chunk.get(2).copied().unwrap_or(0);
            out.push(ALPHA[(b0 >> 2) as usize] as char);
            out.push(ALPHA[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
            if chunk.len() > 1 {
                out.push(ALPHA[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
            } else {
                out.push('=');
            }
            if chunk.len() > 2 {
                out.push(ALPHA[(b2 & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
        out
    }

    fn decode(s: &str) -> Result<Vec<u8>, &'static str> {
        let mut lookup = [255u8; 256];
        for (i, b) in ALPHA.iter().enumerate() {
            lookup[*b as usize] = u8::try_from(i).expect("base64 alphabet index fits in u8");
        }
        let bytes = s.as_bytes();
        if !bytes.len().is_multiple_of(4) {
            return Err("base64 input length not multiple of 4");
        }
        let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
        for chunk in bytes.chunks(4) {
            let v0 = lookup[chunk[0] as usize];
            let v1 = lookup[chunk[1] as usize];
            let v2 = lookup[chunk[2] as usize];
            let v3 = lookup[chunk[3] as usize];
            if v0 == 255 || v1 == 255 {
                return Err("invalid base64 char");
            }
            out.push((v0 << 2) | (v1 >> 4));
            if chunk[2] != b'=' {
                if v2 == 255 {
                    return Err("invalid base64 char");
                }
                out.push((v1 << 4) | (v2 >> 2));
            }
            if chunk[3] != b'=' {
                if v3 == 255 {
                    return Err("invalid base64 char");
                }
                out.push((v2 << 6) | v3);
            }
        }
        Ok(out)
    }

    pub(super) fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&encode(bytes))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        decode(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_wire_format() {
        assert_eq!(WireFormat::parse("proto").unwrap(), WireFormat::Proto);
        assert_eq!(WireFormat::parse("json").unwrap(), WireFormat::Json);
        assert!(WireFormat::parse("xml").is_err());
    }

    #[test]
    fn proto_round_trip_minimal() {
        let req = WorkRequest {
            arguments: vec!["-source".into(), "21".into(), "Foo.java".into()],
            ..WorkRequest::default()
        };
        let bytes = req.encode_framed(WireFormat::Proto).unwrap();
        // Encoding is length-delimited; round-trip via prost's decoder.
        let mut cursor: &[u8] = &bytes;
        let decoded = <WorkRequest as ProstMessage>::decode_length_delimited(&mut cursor).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn json_round_trip_uses_camel_case() {
        let resp = WorkResponse {
            exit_code: 0,
            output: "compiled Foo.java".into(),
            request_id: 0,
            was_cancelled: false,
        };
        let bytes = serde_json::to_vec(&resp).unwrap();
        let s = core::str::from_utf8(&bytes).unwrap();
        // Bazel's JSON convention is camelCase; assert the wire form.
        assert!(s.contains(r#""exitCode":0"#), "got: {s}");
        assert!(s.contains(r#""requestId":0"#), "got: {s}");
        assert!(s.contains(r#""wasCancelled":false"#), "got: {s}");

        let parsed: WorkResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed, resp);
    }

    #[test]
    fn json_decodes_minimum_response() {
        // A real worker may omit fields whose values are proto defaults.
        let bytes = br#"{"exitCode":1,"output":"oh no"}"#;
        let resp = WorkResponse::decode_framed(bytes, WireFormat::Json).unwrap();
        assert_eq!(resp.exit_code, 1);
        assert_eq!(resp.output, "oh no");
        assert_eq!(resp.request_id, 0);
        assert!(!resp.was_cancelled);
    }

    #[test]
    fn proto_request_includes_inputs_with_digest() {
        let req = WorkRequest {
            arguments: vec!["@argfile".into()],
            inputs: vec![Input {
                path: "Foo.java".into(),
                digest: vec![0xde, 0xad, 0xbe, 0xef],
            }],
            ..WorkRequest::default()
        };
        let bytes = req.encode_framed(WireFormat::Proto).unwrap();
        let mut cursor: &[u8] = &bytes;
        let decoded = <WorkRequest as ProstMessage>::decode_length_delimited(&mut cursor).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn json_input_digest_uses_base64() {
        let req = WorkRequest {
            inputs: vec![Input {
                path: "Foo.java".into(),
                digest: vec![0xde, 0xad, 0xbe, 0xef],
            }],
            ..WorkRequest::default()
        };
        let s = serde_json::to_string(&req).unwrap();
        // 0xdeadbeef base64-encodes to "3q2+7w==".
        assert!(s.contains(r#""digest":"3q2+7w==""#), "got: {s}");

        let parsed: WorkRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, req);
    }
}
