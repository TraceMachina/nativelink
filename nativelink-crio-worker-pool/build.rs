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

fn main() -> Result<(), Box<dyn core::error::Error>> {
    // Compile CRI protocol buffers
    tonic_build::configure()
        .build_server(false) // We're a client, not a server
        .build_client(true)
        .compile_protos(&["proto/cri/api.proto"], &["proto/cri"])?;

    // Tell cargo to rerun this build script if the proto file changes
    println!("cargo:rerun-if-changed=proto/cri/api.proto");
    println!("cargo:rerun-if-changed=proto/cri");

    Ok(())
}
