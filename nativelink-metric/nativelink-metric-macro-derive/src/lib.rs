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
    parse_macro_input, Attribute, DeriveInput, Ident, ImplGenerics, LitStr, TypeGenerics,
    WhereClause,
};

/// Holds the type of group for the metric. For example, if a metric
/// has no group it'll be `None`, if it has a static group name it'll
/// be `StaticGroupName(name_of_metric)`.
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

/// Holds the type of the metric. If the metric was not specified
/// it'll be `Default`, which will try to resolve the type from the
/// [`MetricsComponent::publish()`] method that got executed based on
/// the type of the field.
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

/// Holds general information about a specific field that is to be published.
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

/// Holds the template information about the struct. This is needed
/// to create the `MetricsComponent` impl.
#[derive(Debug)]
struct Generics<'a> {
    implementation: ImplGenerics<'a>,
    ty: TypeGenerics<'a>,
    where_clause: Option<&'a WhereClause>,
}

/// Holds metadata about the struct that is having `MetricsComponent`
/// implemented.
#[derive(Debug)]
struct MetricStruct<'a> {
    name: &'a Ident,
    metric_fields: Vec<MetricFieldMetaData<'a>>,
    generics: Generics<'a>,
}

impl ToTokens for MetricStruct<'_> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let name = &self.name;
        let impl_generics = &self.generics.implementation;
        let ty_generics = &self.generics.ty;
        let where_clause = &self.generics.where_clause;

        let metric_fields = self.metric_fields.iter().map(|field| {
            let field_name = &field.field_name;
            let group = &field.group;

            let help = if let Some(help) = field.help.as_ref() {
                quote! { #help }
            } else {
                quote! { "" }
            };
            let value = if let Some(handler) = &field.handler {
                quote! { &#handler(&self.#field_name) }
            } else {
                quote! { &self.#field_name }
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
    let syn::Data::Struct(data) = &input.data else {
        panic!("MetricsComponent can only be derived for structs")
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

    let (implementation, ty, where_clause) = input.generics.split_for_impl();
    let metrics_struct = MetricStruct {
        name: &input.ident,
        metric_fields,
        generics: Generics {
            implementation,
            ty,
            where_clause,
        },
    };
    // This line is intentionally left here to make debugging
    // easier. If you want to see the output of the macro, just
    // uncomment this line and run the tests.
    // panic!("{}", quote! { #metrics_struct });
    TokenStream::from(quote! { #metrics_struct })
}
