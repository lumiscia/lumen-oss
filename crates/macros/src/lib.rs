//! Node schema proc macros.
//!
//! The public proc-macro entry points live here. Expansion code is split into
//! modules so node, parameter, enum, and delegate generation can evolve without
//! turning this crate into one giant parser file again.

use proc_macro::TokenStream;
use syn::parse_macro_input;

mod common;
mod delegate;
mod schema;
mod structural_hash;

/// Hashes every field structurally, including enum variant discriminants.
#[proc_macro_derive(StructuralHash, attributes(structural_hash))]
pub fn derive_structural_hash(input: TokenStream) -> TokenStream {
    match structural_hash::expand(&parse_macro_input!(input as syn::DeriveInput)) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[proc_macro_derive(Node, attributes(node, input, params))]
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
