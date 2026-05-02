//! Proc-macro crate for the Lumen compositing engine node system.
//!
//! Two macros:
//!
//! ## `#[derive(Node)]` — on the struct
//!
//! Processes `#[input]` and `#[property]` field attributes. Generates:
//! - Static `InputPortDef` array + `input_port_defs()` accessor
//! - `resolve_<field>(ctx)` helpers that evaluate `NodeProperty` → concrete type
//! - `PropertyEval` trait implementation
//!
//! ## `#[node_impl]` — on an impl block
//!
//! Processes `#[output]` method attributes. Generates:
//! - Static `OutputPortDef` array + `output_port_defs()` accessor
//! - `Node` trait impl that dispatches `evaluate(ctx, port)` to the matching method
//!
//! ## Example
//!
//! ```ignore
//! #[derive(Node)]
//! pub struct Merge {
//!     pub id: NodeId,
//!
//!     #[property(expected = Float)]
//!     pub opacity: NodeProperty,
//!     #[property(expected = Int)]
//!     pub blend_mode: NodeProperty,
//!
//!     #[input(kind = Raster)]
//!     pub base: NodeRef,
//!     #[input(kind = Raster)]
//!     pub overlay: NodeRef,
//!     #[input(kind = Raster, optional)]
//!     pub mask: NodeRef,
//! }
//!
//! #[node_impl]
//! impl Merge {
//!     #[output(port = "raster_out", kind = Raster)]
//!     fn eval_output(&self, ctx: &mut RenderContext) -> Result<GpuImageFrame, LumenError> {
//!         let opacity = self.resolve_opacity(&ctx.expr_ctx)?;
//!         let base = ctx.eval_node(&self.base)?.as_raster()?;
//!         // ...
//!     }
//! }
//!
//! // Generated Node impl:
//! // impl Node for Merge {
//! //     fn id(&self) -> NodeId { self.id }
//! //     fn evaluate(&self, ctx: &mut RenderContext, port: &str) -> Result<NodeResult, LumenError> {
//! //         match port {
//! //             "raster_out" => self.eval_output(ctx).map(Into::into),
//! //             _ => Err(...)
//! //         }
//! //     }
//! // }
//! ```

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Data, DeriveInput, Fields, Ident, ImplItem, ItemImpl, Lit, Meta, Token,
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
};

// ===========================================================================
// Shared attribute parsing
// ===========================================================================

/// Parameters for `#[input(kind = Raster, optional, variadic)]`.
enum InputAttrParam {
    Kind(Ident),
    Optional,
    Variadic,
    Port(String),
}

impl Parse for InputAttrParam {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let key: Ident = input.parse()?;
        match key.to_string().as_str() {
            "kind" => {
                input.parse::<Token![=]>()?;
                let value: Ident = input.parse()?;
                Ok(InputAttrParam::Kind(value))
            }
            "port" => {
                input.parse::<Token![=]>()?;
                let value: Lit = input.parse()?;
                match value {
                    Lit::Str(s) => Ok(InputAttrParam::Port(s.value())),
                    _ => Err(syn::Error::new_spanned(value, "expected string literal")),
                }
            }
            "optional" => Ok(InputAttrParam::Optional),
            "variadic" => Ok(InputAttrParam::Variadic),
            other => Err(syn::Error::new(
                key.span(),
                format!("unknown input parameter `{other}`"),
            )),
        }
    }
}

struct InputAttrs {
    kind: Ident,
    optional: bool,
    variadic: bool,
    port_name: Option<String>,
}

fn parse_input_attrs(attr: &syn::Attribute) -> syn::Result<InputAttrs> {
    let mut kind: Option<Ident> = None;
    let mut optional = false;
    let mut variadic = false;
    let mut port_name: Option<String> = None;

    if let Meta::List(ref list) = attr.meta {
        let params: Punctuated<InputAttrParam, Token![,]> =
            list.parse_args_with(Punctuated::parse_terminated)?;
        for param in params {
            match param {
                InputAttrParam::Kind(k) => kind = Some(k),
                InputAttrParam::Optional => optional = true,
                InputAttrParam::Variadic => variadic = true,
                InputAttrParam::Port(p) => port_name = Some(p),
            }
        }
    }

    let kind = kind.ok_or_else(|| {
        syn::Error::new_spanned(
            attr,
            "#[input] requires `kind = <PortKind>` (e.g. Raster, Vector)",
        )
    })?;

    Ok(InputAttrs {
        kind,
        optional,
        variadic,
        port_name,
    })
}

/// Parameters for `#[output(port = "name", kind = Raster)]`.
enum OutputAttrParam {
    Kind(Ident),
    Port(String),
}

impl Parse for OutputAttrParam {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let key: Ident = input.parse()?;
        match key.to_string().as_str() {
            "kind" => {
                input.parse::<Token![=]>()?;
                let value: Ident = input.parse()?;
                Ok(OutputAttrParam::Kind(value))
            }
            "port" => {
                input.parse::<Token![=]>()?;
                let value: Lit = input.parse()?;
                match value {
                    Lit::Str(s) => Ok(OutputAttrParam::Port(s.value())),
                    _ => Err(syn::Error::new_spanned(value, "expected string literal")),
                }
            }
            other => Err(syn::Error::new(
                key.span(),
                format!("unknown output parameter `{other}`"),
            )),
        }
    }
}

struct OutputAttrs {
    kind: Ident,
    port_name: Option<String>,
}

fn parse_output_attrs(attr: &syn::Attribute) -> syn::Result<OutputAttrs> {
    let mut kind: Option<Ident> = None;
    let mut port_name: Option<String> = None;

    if let Meta::List(ref list) = attr.meta {
        let params: Punctuated<OutputAttrParam, Token![,]> =
            list.parse_args_with(Punctuated::parse_terminated)?;
        for param in params {
            match param {
                OutputAttrParam::Kind(k) => kind = Some(k),
                OutputAttrParam::Port(p) => port_name = Some(p),
            }
        }
    }

    let kind = kind.ok_or_else(|| {
        syn::Error::new_spanned(
            &attr.meta,
            "#[output] requires `kind = <PortKind>` (e.g. Raster, Vector)",
        )
    })?;

    Ok(OutputAttrs { kind, port_name })
}

/// Parameters for `#[property(expected = Float)]`.
enum PropertyAttrParam {
    Expected(Ident),
}

impl Parse for PropertyAttrParam {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let key: Ident = input.parse()?;
        match key.to_string().as_str() {
            "expected" => {
                input.parse::<Token![=]>()?;
                let value: Ident = input.parse()?;
                Ok(PropertyAttrParam::Expected(value))
            }
            other => Err(syn::Error::new(
                key.span(),
                format!("unknown property parameter `{other}`"),
            )),
        }
    }
}

fn parse_property_attrs(attr: &syn::Attribute) -> syn::Result<Ident> {
    let mut expected: Option<Ident> = None;

    if let Meta::List(ref list) = attr.meta {
        let params: Punctuated<PropertyAttrParam, Token![,]> =
            list.parse_args_with(Punctuated::parse_terminated)?;
        for param in params {
            match param {
                PropertyAttrParam::Expected(e) => expected = Some(e),
            }
        }
    }

    expected.ok_or_else(|| {
        syn::Error::new_spanned(
            attr,
            "#[property] requires `expected = <Type>` (Float, Int, Bool, String, Color, Vec2)",
        )
    })
}

// ===========================================================================
// Collected info
// ===========================================================================

struct InputPort {
    field_ident: Ident,
    name: String,
    kind: Ident,
    optional: bool,
    variadic: bool,
}

struct Property {
    field_ident: Ident,
    name: String,
    expected: Ident,
}

struct OutputMethod {
    port_name: String,
    method_name: Ident,
    kind: Ident,
}

// ===========================================================================
// #[derive(Node)] — struct-level
// ===========================================================================

#[proc_macro_derive(Node, attributes(input, property))]
pub fn derive_node(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match derive_node_inner(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn derive_node_inner(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let struct_name = &input.ident;

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    struct_name,
                    "#[derive(Node)] only supports structs with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                struct_name,
                "#[derive(Node)] only supports structs",
            ));
        }
    };

    let mut id_field: Option<Ident> = None;
    let mut inputs: Vec<InputPort> = Vec::new();
    let mut properties: Vec<Property> = Vec::new();

    for field in fields {
        let field_ident = field.ident.as_ref().unwrap();

        if is_node_id_type(&field.ty) {
            if id_field.is_some() {
                return Err(syn::Error::new_spanned(
                    field_ident,
                    "only one NodeId field is allowed",
                ));
            }
            id_field = Some(field_ident.clone());
        }

        for attr in &field.attrs {
            let path = attr.path();
            if path.is_ident("input") {
                let input_attrs = parse_input_attrs(attr)?;
                let port_name = input_attrs
                    .port_name
                    .unwrap_or_else(|| field_ident.to_string());
                inputs.push(InputPort {
                    field_ident: field_ident.clone(),
                    name: port_name,
                    kind: input_attrs.kind,
                    optional: input_attrs.optional,
                    variadic: input_attrs.variadic,
                });
            } else if path.is_ident("property") {
                let expected = parse_property_attrs(attr)?;
                properties.push(Property {
                    field_ident: field_ident.clone(),
                    name: field_ident.to_string(),
                    expected,
                });
            }
        }
    }

    let id_field = id_field.ok_or_else(|| {
        syn::Error::new_spanned(
            struct_name,
            "#[derive(Node)] requires a field of type `NodeId`",
        )
    })?;

    // --- Input port def statics ---
    let input_count = inputs.len();
    let input_defs = inputs.iter().map(|inp| {
        let name = &inp.name;
        let optional = inp.optional;
        let variadic = inp.variadic;
        let port_kind = port_kind_ident(&inp.kind);
        quote! {
            InputPortDef {
                name: #name,
                kind: PortKind::#port_kind,
                optional: #optional,
                variadic: #variadic,
            }
        }
    });

    let input_static = format_ident!(
        "__{}_INPUT_PORT_DEFS",
        struct_name.to_string().to_uppercase()
    );

    // --- PropertyEval trait: get_property match arms ---
    let property_eval_arms = properties.iter().map(|prop| {
        let name = &prop.name;
        let field = &prop.field_ident;
        quote! { #name => Ok(Some(self.#field.clone())) }
    });

    // --- resolve_<field>() methods ---
    let mut resolve_methods = Vec::new();
    for prop in &properties {
        let field = &prop.field_ident;
        let method_name = format_ident!("resolve_{}", field);
        let prop_name = &prop.name;
        let id_field_ref = &id_field;
        let (return_type, extract_call) = resolve_type_tokens(&prop.expected)?;

        resolve_methods.push(quote! {
            pub fn #method_name<
                '__ctx,
                __S: crate::render::surface::SurfacePool,
                __M: crate::media::MediaStore,
            >(
                &self,
                ctx: &crate::render::context::RenderContext<'__ctx, __S, __M>,
            ) -> Result<#return_type, crate::error::LumenError> {
                let __expr_ctx = ctx.expr_context(
                    format!("{}.{}", self.#id_field_ref, #prop_name),
                );
                self.#field.#extract_call(
                    self.#id_field_ref,
                    #prop_name,
                    &__expr_ctx,
                )
            }
        });
    }

    // --- Property def statics ---
    let property_count = properties.len();
    let property_defs = properties.iter().map(|prop| {
        let name = &prop.name;
        let property_kind = property_kind_ident(&prop.expected);
        quote! {
            PropertyDef {
                name: #name,
                expected: PropertyKind::#property_kind,
            }
        }
    });

    // --- __set_property match arms ---
    let set_property_arms: Vec<_> = properties
        .iter()
        .map(|prop| {
            let name = &prop.name;
            let field = &prop.field_ident;
            quote! { #name => { self.#field = value; true } }
        })
        .collect();

    // --- __wire_input match arms ---
    let wire_input_arms: Vec<_> = inputs
        .iter()
        .map(|inp| {
            let name = &inp.name;
            let field = &inp.field_ident;
            if inp.variadic {
                quote! { #name => { self.#field.push(port_ref); true } }
            } else {
                quote! { #name => { self.#field = port_ref; true } }
            }
        })
        .collect();

    let property_static =
        format_ident!("__{}_PROPERTY_DEFS", struct_name.to_string().to_uppercase());

    let expanded = quote! {
        #[automatically_derived]
        const _: () = {
            use crate::node::{InputPortDef, PropertyDef, PropertyKind, PortKind, NodeId, NodeProperty};
            use crate::error::LumenError;

            static #input_static: [InputPortDef; #input_count] = [
                #(#input_defs),*
            ];

            static #property_static: [PropertyDef; #property_count] = [
                #(#property_defs),*
            ];

            impl #struct_name {
                pub fn __input_port_defs() -> &'static [InputPortDef] {
                    &#input_static
                }

                pub fn __property_defs() -> &'static [PropertyDef] {
                    &#property_static
                }

                fn node_id(&self) -> NodeId {
                    self.#id_field
                }

                /// Set a property by name. Returns `true` if the name matched.
                pub fn __set_property(&mut self, name: &str, value: NodeProperty) -> bool {
                    match name {
                        #(#set_property_arms)*
                        _ => false,
                    }
                }

                /// Wire an input port by name. Returns `true` if the name matched.
                pub fn __wire_input(&mut self, name: &str, port_ref: crate::node::PortRef) -> bool {
                    match name {
                        #(#wire_input_arms)*
                        _ => false,
                    }
                }

                #(#resolve_methods)*
            }

            impl crate::node::PropertyEval for #struct_name {
                fn property_defs(&self) -> &'static [PropertyDef] {
                    &#property_static
                }

                fn get_property(&self, id: &str) -> Result<Option<NodeProperty>, LumenError> {
                    match id {
                        #(#property_eval_arms,)*
                        _ => Ok(None),
                    }
                }
            }
        };
    };

    Ok(expanded)
}

// ===========================================================================
// #[node_impl] — impl-block level
// ===========================================================================

/// Attribute macro placed on an `impl MyNode { ... }` block.
///
/// Scans for `#[output(port = "name", kind = Raster)]` on methods,
/// then generates:
/// - Static `OutputPortDef` array + `output_port_defs()` accessor
/// - `Node` trait impl with `id()` + `evaluate()` port dispatch
///
/// The original methods are preserved as-is (the `#[output]` attr is stripped).
#[proc_macro_attribute]
pub fn node_impl(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemImpl);
    match node_impl_inner(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn node_impl_inner(mut input: ItemImpl) -> syn::Result<proc_macro2::TokenStream> {
    let struct_name = match &*input.self_ty {
        syn::Type::Path(type_path) => type_path
            .path
            .segments
            .last()
            .map(|s| s.ident.clone())
            .ok_or_else(|| syn::Error::new_spanned(&input.self_ty, "expected a type name"))?,
        _ => {
            return Err(syn::Error::new_spanned(
                &input.self_ty,
                "#[node_impl] requires a named type",
            ));
        }
    };

    let mut output_methods: Vec<OutputMethod> = Vec::new();

    // Walk methods, collect #[output] attrs, then strip them
    for item in &mut input.items {
        if let ImplItem::Fn(method) = item {
            let mut output_attr_index = None;

            for (i, attr) in method.attrs.iter().enumerate() {
                if attr.path().is_ident("output") {
                    let out_attrs = parse_output_attrs(attr)?;
                    let port_name = out_attrs
                        .port_name
                        .unwrap_or_else(|| method.sig.ident.to_string());

                    output_methods.push(OutputMethod {
                        port_name,
                        method_name: method.sig.ident.clone(),
                        kind: out_attrs.kind,
                    });

                    output_attr_index = Some(i);
                    break;
                }
            }

            // Strip the #[output] attr so it doesn't confuse the compiler
            if let Some(i) = output_attr_index {
                method.attrs.remove(i);
            }
        }
    }

    // Rewrite `RenderContext` types in method signatures and add method-level generics.
    // We can't put generics on the inherent impl block (Rust requires them to appear in the
    // self type), so each method that mentions `RenderContext` gets its own generic params.
    for item in &mut input.items {
        if let ImplItem::Fn(method) = item {
            let mut needs_generics = false;
            for arg in &mut method.sig.inputs {
                if let syn::FnArg::Typed(pat_type) = arg {
                    if type_contains_ident(&pat_type.ty, "RenderContext") {
                        needs_generics = true;
                    }
                    rewrite_render_context(&mut pat_type.ty);
                }
            }
            if needs_generics {
                method
                    .sig
                    .generics
                    .params
                    .insert(0, syn::parse_quote! { '__node_lt });
                method
                    .sig
                    .generics
                    .params
                    .push(syn::parse_quote! { __S: crate::render::surface::SurfacePool });
                method
                    .sig
                    .generics
                    .params
                    .push(syn::parse_quote! { __M: crate::media::MediaStore });
            }
        }
    }

    // --- Output port def statics ---
    let output_count = output_methods.len();
    let output_defs = output_methods.iter().map(|out| {
        let name = &out.port_name;
        let port_kind = port_kind_ident(&out.kind);
        quote! {
            OutputPortDef {
                name: #name,
                kind: PortKind::#port_kind,
            }
        }
    });

    let output_static = format_ident!(
        "__{}_OUTPUT_PORT_DEFS",
        struct_name.to_string().to_uppercase()
    );

    // --- Node trait: evaluate() match arms ---
    let eval_match_arms = output_methods.iter().map(|out| {
        let port_name = &out.port_name;
        let method = &out.method_name;
        quote! {
            #port_name => self.#method(ctx).map(Into::into)
        }
    });

    let node_trait_impl = quote! {
        #[automatically_derived]
        const _: () = {
            use crate::node::{InputPortDef, OutputPortDef, PropertyDef, PropertyKind, PortKind, NodeId, NodeResult, NodeDef};
            use crate::error::LumenError;

            static #output_static: [OutputPortDef; #output_count] = [
                #(#output_defs),*
            ];

            impl #struct_name {
                pub fn __output_port_defs() -> &'static [OutputPortDef] {
                    &#output_static
                }
            }

            // Override the placeholder output_port_defs from #[derive(Node)]
            impl crate::node::NodeDef for #struct_name {
                fn property_defs() -> &'static [PropertyDef] {
                    #struct_name::__property_defs()
                }

                fn input_port_defs() -> &'static [InputPortDef] {
                    #struct_name::__input_port_defs()
                }

                fn output_port_defs() -> &'static [OutputPortDef] {
                    &#output_static
                }
            }

            impl crate::node::Node for #struct_name {
                fn id(&self) -> NodeId {
                    self.node_id()
                }

                fn input_port_defs(&self) -> &'static [InputPortDef] {
                    <#struct_name as NodeDef>::input_port_defs()
                }

                fn output_port_defs(&self) -> &'static [OutputPortDef] {
                    <#struct_name as NodeDef>::output_port_defs()
                }
            }

            impl<'__node_lt, __S: crate::render::surface::SurfacePool, __M: crate::media::MediaStore>
                crate::node::NodeEval<'__node_lt, __S, __M> for #struct_name
            {
                fn evaluate(
                    &self,
                    ctx: &mut crate::render::RenderContext<'__node_lt, __S, __M>,
                    port: &str,
                ) -> Result<NodeResult, LumenError> {
                    match port {
                        #(#eval_match_arms,)*
                        _ => Err(LumenError::Property(
                            crate::error::PropertyError::MissingProperty {
                                node_id: self.node_id(),
                                property_path: format!("output port `{}`", port),
                            }
                        ))
                    }
                }
            }
        };
    };

    // Emit the original impl block (with #[output] stripped) + the generated trait impl
    Ok(quote! {
        #input
        #node_trait_impl
    })
}

/// Check if a type tree contains a path segment with the given identifier.
fn type_contains_ident(ty: &syn::Type, name: &str) -> bool {
    match ty {
        syn::Type::Reference(r) => type_contains_ident(&r.elem, name),
        syn::Type::Path(p) => p.path.segments.iter().any(|seg| {
            if seg.ident == name {
                return true;
            }
            if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                for arg in &args.args {
                    if let syn::GenericArgument::Type(inner) = arg {
                        if type_contains_ident(inner, name) {
                            return true;
                        }
                    }
                }
            }
            false
        }),
        _ => false,
    }
}

/// Recursively walks a type tree and rewrites any `RenderContext` path segment
/// to `RenderContext<'__node_lt, __S, __M>`, replacing existing generic args if present.
fn rewrite_render_context(ty: &mut syn::Type) {
    match ty {
        syn::Type::Reference(r) => rewrite_render_context(&mut r.elem),
        syn::Type::Path(p) => {
            for seg in &mut p.path.segments {
                if seg.ident == "RenderContext" {
                    seg.arguments = syn::PathArguments::AngleBracketed(
                        syn::parse_quote! { <'__node_lt, __S, __M> },
                    );
                }
                // Recurse into generic args of other types (e.g. Option<&mut RenderContext>)
                if let syn::PathArguments::AngleBracketed(args) = &mut seg.arguments {
                    for arg in &mut args.args {
                        if let syn::GenericArgument::Type(inner) = arg {
                            rewrite_render_context(inner);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Returns `(return_type, method_name_on_NodeProperty)` for each expected type.
fn resolve_type_tokens(expected: &Ident) -> syn::Result<(proc_macro2::TokenStream, Ident)> {
    let s = expected.to_string();
    match s.as_str() {
        "Float" => Ok((quote! { f64 }, Ident::new("resolve_float", expected.span()))),
        "Int" => Ok((quote! { i64 }, Ident::new("resolve_int", expected.span()))),
        "Bool" | "Boolean" => Ok((quote! { bool }, Ident::new("resolve_bool", expected.span()))),
        "String" => Ok((
            quote! { String },
            Ident::new("resolve_string", expected.span()),
        )),
        "Color" => Ok((
            quote! { [u8; 4] },
            Ident::new("resolve_color", expected.span()),
        )),
        "Vec2" | "Vector2" => Ok((
            quote! { (f64, f64) },
            Ident::new("resolve_vec2", expected.span()),
        )),
        _ => Err(syn::Error::new(
            expected.span(),
            format!(
                "unknown property type `{s}`, expected one of: Float, Int, Bool, String, Color, Vec2"
            ),
        )),
    }
}

/// Map shorthand kind identifiers to `PortKind` variant names.
fn port_kind_ident(ident: &Ident) -> Ident {
    let s = ident.to_string();
    match s.as_str() {
        "Raster" | "GpuImageFrame" => Ident::new("GpuImageFrame", ident.span()),
        "Vector" => Ident::new("Vector", ident.span()),
        "Surface" => Ident::new("Surface", ident.span()),
        _ => ident.clone(),
    }
}

/// Map expected type identifiers to `PropertyKind` variant names.
fn property_kind_ident(ident: &Ident) -> Ident {
    let s = ident.to_string();
    match s.as_str() {
        "Float" => Ident::new("Float", ident.span()),
        "Int" => Ident::new("Int", ident.span()),
        "Bool" | "Boolean" => Ident::new("Bool", ident.span()),
        "String" => Ident::new("String", ident.span()),
        "Color" => Ident::new("Color", ident.span()),
        "Vec2" | "Vector2" => Ident::new("Vec2", ident.span()),
        _ => ident.clone(),
    }
}

/// Check if a type path ends with `NodeId`.
fn is_node_id_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty {
        if let Some(last) = type_path.path.segments.last() {
            return last.ident == "NodeId";
        }
    }
    false
}
