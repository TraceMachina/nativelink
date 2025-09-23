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

use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, parse_macro_input};

#[proc_macro_attribute]
pub fn nativelink_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = proc_macro2::TokenStream::from(attr);
    let input_fn = parse_macro_input!(item as ItemFn);

    let fn_name = &input_fn.sig.ident;
    let fn_block = &input_fn.block;
    let fn_inputs = &input_fn.sig.inputs;
    let fn_output = &input_fn.sig.output;
    let fn_attr = &input_fn.attrs;

    let expanded = quote! {
        #(#fn_attr)*
        #[expect(
            clippy::disallowed_methods,
            reason = "`tokio::test` uses `tokio::runtime::Runtime::block_on`"
        )]
        #[tokio::test(#attr)]
        #[::tracing_test::traced_test]
        async fn #fn_name(#fn_inputs) #fn_output {
            ::nativelink_util::__tracing::error_span!(stringify!(#fn_name))
                .in_scope(|| async move {
                    ::nativelink_util::common::reseed_rng_for_test().unwrap();
                    let res = #fn_block;
                    logs_assert(|lines: &[&str]| {
                        for line in lines {
                            if line.contains(" data: b") {
                                return Err(format!("Non-redacted data in \"{line}\""));
                            }
                        }
                        Ok(())
                    });
                    res
                })
                .await
        }
    };

    TokenStream::from(expanded)
}
