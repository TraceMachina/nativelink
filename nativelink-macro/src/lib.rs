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
use proc_macro2::TokenTree;
use quote::{format_ident, quote};
use syn::{ItemFn, parse_macro_input};

// Helper function for debugging. Add prettyplease as dependency
//
// fn unparse(input: proc_macro2::TokenStream) -> String {
//     let item = syn::parse2(input).unwrap();
//     let file = syn::File {
//         attrs: vec![],
//         items: vec![item],
//         shebang: None,
//     };

//     prettyplease::unparse(&file)
// }

// Either use this as-is or as `#[nativelink_test("foo")]` where foo is the path for nativelink-util
// Mostly used inside nativelink-util as `#[nativelink_test("crate")]`
// If you start it with an ident instead, e.g. `#[nativelink_test(flavor = "multi_thread")]` we feed it into tokio::test
#[proc_macro_attribute]
pub fn nativelink_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = proc_macro2::TokenStream::from(attr);
    let input_fn = parse_macro_input!(item as ItemFn);
    let mut maybe_crate_ident: Option<proc_macro2::TokenStream> = None;
    let mut maybe_tokio_attrs: Option<proc_macro2::TokenStream> = None;

    for a in attr.clone() {
        assert!(maybe_crate_ident.is_none());

        match a {
            TokenTree::Literal(l) => {
                let s = format_ident!("{}", l.to_string().replace('"', ""));
                maybe_crate_ident = Some(quote! {#s});
            }
            TokenTree::Ident(_) => {
                maybe_tokio_attrs = Some(attr);
                break;
            }
            _ => {
                panic!("unsupported tokentree: {a:?}");
            }
        }
    }

    let fn_name = &input_fn.sig.ident;
    let fn_block = &input_fn.block;
    let fn_inputs = &input_fn.sig.inputs;
    let fn_output = &input_fn.sig.output;
    let fn_attr = &input_fn.attrs;
    let crate_ident = maybe_crate_ident.unwrap_or_else(|| quote!(::nativelink_util));
    let tokio_attrs = maybe_tokio_attrs.unwrap_or_else(|| quote!());

    let expanded = quote! {
        #(#fn_attr)*
        #[expect(
            clippy::disallowed_methods,
            reason = "`tokio::test` uses `tokio::runtime::Runtime::block_on`"
        )]
        #[tokio::test(#tokio_attrs)]
        #[::tracing_test::traced_test]
        async fn #fn_name(#fn_inputs) #fn_output {
            #crate_ident::__tracing::error_span!(stringify!(#fn_name))
                .in_scope(|| async move {
                    #crate_ident::common::reseed_rng_for_test().unwrap();
                    let res = #fn_block;
                    logs_assert(|lines: &[&str]| {
                        for line in lines {
                            // Most cases of this are us failing to skip something from debug
                            // But aws_runtime also has this, so ignore that
                            if line.contains(" data: b") && !line.contains("aws_runtime::content_encoding::body::http_body_1_x") {
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
