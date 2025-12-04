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
    // CRI-O requires Unix domain sockets - skip build on Windows
    #[cfg(not(unix))]
    {
        println!(
            "cargo:warning=nativelink-crio-worker-pool is Unix-only and will not be built on this platform"
        );
        return Ok(());
    }

    #[cfg(unix)]
    {
        // Compile CRI protocol buffers with linter suppressions for generated code
        tonic_build::configure()
            .build_server(false) // We're a client, not a server
            .build_client(true)
            .emit_rerun_if_changed(false) // We handle this manually below
            .type_attribute(".", "#[allow(clippy::all, unused_qualifications)]")
            .compile_protos(&["proto/cri/api.proto"], &["proto/cri"])?;

        // Tell cargo to rerun this build script if the proto file changes
        println!("cargo:rerun-if-changed=proto/cri/api.proto");
        println!("cargo:rerun-if-changed=proto/cri");

        Ok(())
    } // End cfg(unix)
}
