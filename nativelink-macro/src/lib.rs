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

extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn};

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
        #[allow(clippy::disallowed_methods)]
        #[tokio::test(#attr)]
        async fn #fn_name(#fn_inputs) #fn_output {
            // Error means already initialized, which is ok.
            let _ = nativelink_util::init_tracing();
            // If already set it's ok.
            let _ = nativelink_util::fs::set_idle_file_descriptor_timeout(std::time::Duration::from_millis(100));

            #[warn(clippy::disallowed_methods)]
            ::std::sync::Arc::new(::nativelink_util::origin_context::OriginContext::new()).wrap_async(
                ::nativelink_util::__tracing::trace_span!("test"), async move {
                    #fn_block
                }
            )
            .await
        }
    };

    TokenStream::from(expanded)
}
