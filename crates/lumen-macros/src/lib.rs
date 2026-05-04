//! Node schema proc macros.
//!
//! `#[derive(Node)]` turns a renderer-neutral node struct into canonical
//! schema metadata plus property lookup glue. It intentionally does not model
//! UI concerns or multiple outputs.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::{
    Attribute, Data, DeriveInput, Fields, Ident, ImplItem, ItemImpl, LitBool, LitStr, Result,
    Token, parse_macro_input, punctuated::Punctuated, spanned::Spanned,
};

#[proc_macro_derive(Node, attributes(node, input, property))]
pub fn derive_node(input: TokenStream) -> TokenStream {
    match expand_node(parse_macro_input!(input as DeriveInput)) {
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

fn expand_node(input: DeriveInput) -> Result<proc_macro2::TokenStream> {
    let ident = input.ident;
    let node = parse_node_attr(&input.attrs)?;
    let fields = named_fields(&input.data)?;
    let mut inputs = Vec::new();
    let mut properties = Vec::new();

    for field in fields {
        let Some(field_ident) = &field.ident else {
            continue;
        };
        if let Some(input) = parse_input_attr(&field.attrs, field_ident)? {
            inputs.push(input);
        }
        if let Some(property) = parse_property_attr(&field.attrs, field_ident)? {
            properties.push(property);
        }
    }

    let input_defs = inputs.iter().map(|input| {
        let name = &input.name;
        let kind = port_kind_tokens(&input.kind);
        let optional = input.optional;
        let variadic = input.variadic;
        quote! {
            ::lumen::node::InputPortDef {
                name: #name,
                kind: #kind,
                optional: #optional,
                variadic: #variadic,
            }
        }
    });
    let property_defs = properties.iter().map(|property| {
        let name = &property.name;
        let kind = property_kind_tokens(&property.kind);
        quote! {
            ::lumen::node::PropertyDef {
                name: #name,
                expected: #kind,
            }
        }
    });
    let default_properties = properties.iter().map(|property| {
        let name = &property.name;
        let field = &property.field;
        quote!((#name, defaults.#field.clone()))
    });
    let property_matches = properties.iter().map(|property| {
        let name = &property.name;
        let field = &property.field;
        quote!(#name => Some(self.#field.clone()))
    });

    let input_static = format_ident!("__LUMEN_{}_INPUTS", ident.to_string().to_uppercase());
    let property_static = format_ident!("__LUMEN_{}_PROPERTIES", ident.to_string().to_uppercase());
    let kind = node.kind;
    let label = node.label;
    let description = node.description;
    let category = category_tokens(&node.category);

    Ok(quote! {
        const #input_static: &[::lumen::node::InputPortDef] = &[#(#input_defs),*];
        const #property_static: &[::lumen::node::PropertyDef] = &[#(#property_defs),*];

        impl ::lumen::node::NodeSchema for #ident {
            fn schema() -> ::lumen::node::NodeSchemaDef {
                let defaults = <Self as ::core::default::Default>::default();
                ::lumen::node::NodeSchemaDef {
                    kind: #kind,
                    label: #label,
                    description: #description,
                    category: #category,
                    inputs: #input_static,
                    properties: #property_static,
                    default_properties: vec![#(#default_properties),*],
                }
            }
        }

        impl ::lumen::node::Node for #ident {
            fn id(&self) -> ::lumen::node::NodeId {
                self.id
            }

            fn input_port_defs(&self) -> &'static [::lumen::node::InputPortDef] {
                #input_static
            }
        }

        impl ::lumen::node::PropertyEval for #ident {
            fn get_property(
                &self,
                id: &str,
            ) -> ::core::result::Result<
                ::core::option::Option<::lumen::node::NodeProperty>,
                ::lumen::error::LumenError,
            > {
                Ok(match id {
                    #(#property_matches,)*
                    _ => None,
                })
            }
        }
    })
}

fn named_fields(data: &Data) -> Result<&Punctuated<syn::Field, Token![,]>> {
    match data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => Ok(&fields.named),
            fields => Err(syn::Error::new(
                fields.span(),
                "Node derive requires named struct fields",
            )),
        },
        _ => Err(syn::Error::new(
            Span::call_site(),
            "Node derive only supports structs",
        )),
    }
}

struct NodeAttr {
    kind: LitStr,
    label: LitStr,
    description: LitStr,
    category: Ident,
}

struct InputAttr {
    name: LitStr,
    kind: Ident,
    optional: bool,
    variadic: bool,
}

struct PropertyAttr {
    field: Ident,
    name: LitStr,
    kind: Ident,
}

fn parse_node_attr(attrs: &[Attribute]) -> Result<NodeAttr> {
    let attr = required_attr(attrs, "node")?;
    let mut kind = None;
    let mut label = None;
    let mut description = None;
    let mut category = None;

    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("kind") {
            kind = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("label") {
            label = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("description") {
            description = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("category") {
            category = Some(ident_value(&meta)?);
        } else {
            return Err(meta.error("unknown node attribute key"));
        }
        Ok(())
    })?;

    Ok(NodeAttr {
        kind: required_value(kind, attr, "kind")?,
        label: required_value(label, attr, "label")?,
        description: required_value(description, attr, "description")?,
        category: required_value(category, attr, "category")?,
    })
}

fn parse_input_attr(attrs: &[Attribute], field: &Ident) -> Result<Option<InputAttr>> {
    let Some(attr) = optional_attr(attrs, "input") else {
        return Ok(None);
    };
    let mut name = None;
    let mut kind = None;
    let mut optional = None;
    let mut variadic = None;

    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("name") {
            name = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("kind") {
            kind = Some(ident_value(&meta)?);
        } else if meta.path.is_ident("optional") {
            optional = Some(optional_bool(&meta)?);
        } else if meta.path.is_ident("variadic") {
            variadic = Some(optional_bool(&meta)?);
        } else {
            return Err(meta.error("unknown input attribute key"));
        }
        Ok(())
    })?;

    Ok(Some(InputAttr {
        name: name.unwrap_or_else(|| LitStr::new(&field.to_string(), field.span())),
        kind: kind.unwrap_or_else(|| Ident::new("raster", Span::call_site())),
        optional: optional.unwrap_or(false),
        variadic: variadic.unwrap_or(false),
    }))
}

fn parse_property_attr(attrs: &[Attribute], field: &Ident) -> Result<Option<PropertyAttr>> {
    let Some(attr) = optional_attr(attrs, "property") else {
        return Ok(None);
    };
    let mut name = None;
    let mut kind = None;

    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("name") {
            name = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("kind") {
            kind = Some(ident_value(&meta)?);
        } else {
            return Err(meta.error("unknown property attribute key"));
        }
        Ok(())
    })?;

    Ok(Some(PropertyAttr {
        field: field.clone(),
        name: name.unwrap_or_else(|| LitStr::new(&field.to_string(), field.span())),
        kind: required_value(kind, attr, "kind")?,
    }))
}

fn required_attr<'a>(attrs: &'a [Attribute], name: &str) -> Result<&'a Attribute> {
    optional_attr(attrs, name)
        .ok_or_else(|| syn::Error::new(Span::call_site(), format!("missing #[{name}(...)]")))
}

fn optional_attr<'a>(attrs: &'a [Attribute], name: &str) -> Option<&'a Attribute> {
    attrs.iter().find(|attr| attr.path().is_ident(name))
}

fn required_value<T>(value: Option<T>, attr: &Attribute, name: &str) -> Result<T> {
    value.ok_or_else(|| {
        syn::Error::new(
            attr.span(),
            format!(
                "missing `{name}` in #[{}(...)]",
                attr.path().get_ident().unwrap()
            ),
        )
    })
}

fn ident_value(meta: &syn::meta::ParseNestedMeta<'_>) -> Result<Ident> {
    let value = meta.value()?;
    if value.peek(LitStr) {
        let lit: LitStr = value.parse()?;
        Ok(Ident::new(&lit.value(), lit.span()))
    } else {
        value.parse()
    }
}

fn optional_bool(meta: &syn::meta::ParseNestedMeta<'_>) -> Result<bool> {
    if meta.input.peek(Token![=]) {
        Ok(meta.value()?.parse::<LitBool>()?.value)
    } else {
        Ok(true)
    }
}

fn category_tokens(category: &Ident) -> proc_macro2::TokenStream {
    match category.to_string().as_str() {
        "compositing" | "Compositing" => quote!(::lumen::node::NodeCategory::Compositing),
        "processing" | "Processing" => quote!(::lumen::node::NodeCategory::Processing),
        "source" | "Source" => quote!(::lumen::node::NodeCategory::Source),
        "output" | "Output" => quote!(::lumen::node::NodeCategory::Output),
        "vector" | "Vector" => quote!(::lumen::node::NodeCategory::Vector),
        _ => {
            let span = category.span();
            quote::quote_spanned!(span=> compile_error!("unknown node category"))
        }
    }
}

fn port_kind_tokens(kind: &Ident) -> proc_macro2::TokenStream {
    match kind.to_string().as_str() {
        "raster" | "Raster" => quote!(::lumen::node::PortKind::Raster),
        "vector" | "Vector" => quote!(::lumen::node::PortKind::Vector),
        _ => {
            let span = kind.span();
            quote::quote_spanned!(span=> compile_error!("unknown port kind"))
        }
    }
}

fn property_kind_tokens(kind: &Ident) -> proc_macro2::TokenStream {
    match kind.to_string().as_str() {
        "float" | "Float" => quote!(::lumen::node::PropertyKind::Float),
        "int" | "Int" => quote!(::lumen::node::PropertyKind::Int),
        "bool" | "Bool" => quote!(::lumen::node::PropertyKind::Bool),
        "string" | "String" => quote!(::lumen::node::PropertyKind::String),
        "color" | "Color" => quote!(::lumen::node::PropertyKind::Color),
        "vec2" | "Vec2" => quote!(::lumen::node::PropertyKind::Vec2),
        _ => {
            let span = kind.span();
            quote::quote_spanned!(span=> compile_error!("unknown property kind"))
        }
    }
}
