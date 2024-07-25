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

use core::panic;

use proc_macro::TokenStream;
use quote::{format_ident, quote, ToTokens};
use syn::parse::{Parse, ParseStream};
use syn::{
    parse_macro_input, Attribute, DeriveInput, Ident, ImplGenerics, ItemFn, LitStr, TypeGenerics,
    WhereClause,
};

#[derive(Default, Debug)]
enum GroupType {
    #[default]
    None,
    StaticGroupName(Ident),
}

impl Parse for GroupType {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.is_empty() {
            return Ok(GroupType::None);
        }
        let group_str: LitStr = input.parse()?;
        let group = format_ident!("{}", group_str.value());
        Ok(GroupType::StaticGroupName(group))
    }
}

impl ToTokens for GroupType {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        match self {
            GroupType::None => {
                quote! { "" }
            }
            GroupType::StaticGroupName(group) => quote! { stringify!(#group) },
        }
        .to_tokens(tokens);
    }
}

#[derive(Debug)]
enum MetricKind {
    Default,
    Counter,
    String,
    Component,
}

impl Parse for MetricKind {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let kind_str: LitStr = input.parse()?;
        match kind_str.value().as_str() {
            "counter" => Ok(MetricKind::Counter),
            "string" => Ok(MetricKind::String),
            "component" => Ok(MetricKind::Component),
            "default" => Ok(MetricKind::Default),
            _ => Err(syn::Error::new(kind_str.span(), "Invalid metric type")),
        }
    }
}

impl ToTokens for MetricKind {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        match self {
            MetricKind::Counter => quote! { ::nativelink_metric::MetricKind::Counter },
            MetricKind::String => quote! { ::nativelink_metric::MetricKind::String },
            MetricKind::Component => quote! { ::nativelink_metric::MetricKind::Component },
            MetricKind::Default => quote! { ::nativelink_metric::MetricKind::Default },
        }
        .to_tokens(tokens);
    }
}

#[derive(Debug)]
struct MetricFieldMetaData<'a> {
    field_name: &'a Ident,
    metric_kind: MetricKind,
    help: Option<LitStr>,
    group: GroupType,
    handler: Option<syn::ExprPath>,
}

impl<'a> MetricFieldMetaData<'a> {
    fn try_from(field_name: &'a Ident, attr: &Attribute) -> syn::Result<Self> {
        let mut result = MetricFieldMetaData {
            field_name,
            metric_kind: MetricKind::Default,
            help: None,
            group: GroupType::None,
            handler: None,
        };
        // If the attribute is just a path, it has no args, so use defaults.
        if let syn::Meta::Path(_) = attr.meta {
            return Ok(result);
        }
        attr.parse_args_with(syn::meta::parser(|meta| {
            if meta.path.is_ident("help") {
                result.help = meta.value()?.parse()?;
            } else if meta.path.is_ident("kind") {
                result.metric_kind = meta.value()?.parse()?;
            } else if meta.path.is_ident("group") {
                result.group = meta.value()?.parse()?;
            } else if meta.path.is_ident("handler") {
                result.handler = Some(meta.value()?.parse()?);
            }
            Ok(())
        }))?;
        Ok(result)
    }
}

#[derive(Debug)]
struct Generics<'a> {
    impl_generics: ImplGenerics<'a>,
    ty_generics: TypeGenerics<'a>,
    where_clause: Option<&'a WhereClause>,
}

#[derive(Debug)]
struct MetricStruct<'a> {
    name: &'a Ident,
    metric_fields: Vec<MetricFieldMetaData<'a>>,
    generics: Generics<'a>,
}

impl<'a> ToTokens for MetricStruct<'a> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let name = &self.name;
        let impl_generics = &self.generics.impl_generics;
        let ty_generics = &self.generics.ty_generics;
        let where_clause = &self.generics.where_clause;

        let metric_fields = self.metric_fields.iter().map(|field| {
            let field_name = &field.field_name;
            let group = &field.group;

            let help = match field.help.as_ref() {
                Some(help) => quote! { #help },
                None => quote! { "" },
            };
            let value = match &field.handler {
                Some(handler) => quote! { &#handler(&self.#field_name) },
                None => quote! { &self.#field_name },
            };
            let metric_kind = &field.metric_kind;
            quote! {
                ::nativelink_metric::publish!(
                    stringify!(#field_name),
                    #value,
                    #metric_kind,
                    #help,
                    #group
                );
            }
        });
        quote! {
            impl #impl_generics ::nativelink_metric::MetricsComponent for #name #ty_generics #where_clause {
                fn publish(&self, kind: ::nativelink_metric::MetricKind, field_metadata: ::nativelink_metric::MetricFieldData) -> Result<::nativelink_metric::MetricPublishKnownKindData, ::nativelink_metric::Error> {
                    #( #metric_fields )*
                    Ok(::nativelink_metric::MetricPublishKnownKindData::Component)
                }
            }
        }.to_tokens(tokens);
    }
}

#[proc_macro_derive(MetricsComponent, attributes(metric))]
pub fn metrics_component_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let data = match &input.data {
        syn::Data::Struct(data) => data,
        _ => panic!("MetricsComponent can only be derived for structs"),
    };

    let mut metric_fields = vec![];
    match &data.fields {
        syn::Fields::Named(fields) => {
            fields.named.iter().for_each(|field| {
                field.attrs.iter().for_each(|attr| {
                    if attr.path().is_ident("metric") {
                        metric_fields.push(
                            MetricFieldMetaData::try_from(field.ident.as_ref().unwrap(), attr)
                                .unwrap(),
                        );
                    }
                });
            });
        }
        syn::Fields::Unnamed(_) => {
            panic!("Unnamed fields are not supported");
        }
        syn::Fields::Unit => {
            panic!("Unit structs are not supported");
        }
    }

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let metrics_struct = MetricStruct {
        name: &input.ident,
        metric_fields,
        generics: Generics {
            impl_generics,
            ty_generics,
            where_clause,
        },
    };
    // panic!("{}", quote! { #metrics_struct });
    TokenStream::from(quote! { #metrics_struct })
}

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
            let _ = nativelink_macro::init_tracing();
            // If already set it's ok.
            let _ = nativelink_macro::fs::set_idle_file_descriptor_timeout(std::time::Duration::from_millis(100));

            #[warn(clippy::disallowed_methods)]
            ::std::sync::Arc::new(::nativelink_macro::origin_context::OriginContext::new()).wrap_async(
                ::nativelink_macro::__tracing::trace_span!("test"), async move {
                    #fn_block
                }
            )
            .await
        }
    };

    TokenStream::from(expanded)
}
