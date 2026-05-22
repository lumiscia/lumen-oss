//! Node schema proc macros.
//!
//! The public proc-macro entry points live here. Expansion code is split into
//! modules so node, parameter, enum, and delegate generation can evolve without
//! turning this crate into one giant parser file again.

use proc_macro::TokenStream;
use quote::quote;
use syn::{ImplItem, ItemImpl, parse_macro_input};

mod delegate;
mod schema;

#[proc_macro_derive(Node, attributes(node, input, property, params))]
pub fn derive_node(input: TokenStream) -> TokenStream {
    match schema::expand_node(parse_macro_input!(input as syn::DeriveInput)) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[proc_macro_derive(NodeEnum, attributes(enum_option))]
pub fn derive_node_enum(input: TokenStream) -> TokenStream {
    match schema::expand_node_enum(parse_macro_input!(input as syn::DeriveInput)) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[proc_macro_derive(Delegate, attributes(delegate, meta, default))]
pub fn derive_delegate(input: TokenStream) -> TokenStream {
    match delegate::expand_delegate(parse_macro_input!(input as syn::DeriveInput)) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn node_impl(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(item as ItemImpl);
    for item in &mut input.items {
        if let ImplItem::Fn(method) = item {
            method.attrs.retain(|attr| !attr.path().is_ident("output"));
        }
    }
    quote!(#input).into()
}
