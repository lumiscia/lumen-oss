//! Node schema proc macros.
//!
//! `#[derive(Node)]` turns a renderer-neutral node struct into canonical
//! schema metadata plus property lookup glue. It intentionally does not model
//! UI concerns or multiple outputs.

use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::{
    Attribute, Data, DeriveInput, Expr, ExprLit, Fields, Ident, LitInt, LitStr, Path, Result,
    Token, punctuated::Punctuated, spanned::Spanned,
};

use crate::common::{
    PropertyConstraintsAttr, PropertyDefTokens, doc_lines, doc_string, ident_value, optional_attr,
    optional_bool, property_kind_tokens, title_from_ident, to_snake_case,
};

mod node;
mod node_enum;

pub(crate) use node::expand_node;
pub(crate) use node_enum::expand_node_enum;

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
    name: LitStr,
    category: Ident,
    docs: Vec<String>,
}

struct InputAttr {
    field: Ident,
    name: LitStr,
    kind: Ident,
    optional: bool,
    variadic: bool,
}

struct PropertyAttr {
    field: Ident,
    id: LitStr,
    name: LitStr,
    kind: Ident,
    enum_type: Option<Path>,
    constraints: PropertyConstraintsAttr,
    docs: Vec<String>,
}

struct EnumOptionAttr {
    name: String,
    label: String,
    value: i64,
}

fn parse_node_attr(attrs: &[Attribute]) -> Result<NodeAttr> {
    let attr = required_attr(attrs, "node")?;
    let mut kind = None;
    let mut name = None;
    let mut category = None;

    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("kind") {
            kind = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("name") {
            name = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("category") {
            category = Some(ident_value(&meta)?);
        } else {
            return Err(meta.error("unknown node attribute key"));
        }
        Ok(())
    })?;

    Ok(NodeAttr {
        kind: required_value(kind, attr, "kind")?,
        name: required_value(name, attr, "name")?,
        category: required_value(category, attr, "category")?,
        docs: doc_lines(attrs),
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
        field: field.clone(),
        name: name.unwrap_or_else(|| LitStr::new(&field.to_string(), field.span())),
        kind: kind.unwrap_or_else(|| Ident::new("raster", Span::call_site())),
        optional: optional.unwrap_or(false),
        variadic: variadic.unwrap_or(false),
    }))
}

fn parse_property_attr(attrs: &[Attribute], field: &Ident) -> Result<Option<PropertyAttr>> {
    parse_property_like_attr(attrs, field, "property")
}

fn parse_property_like_attr(
    attrs: &[Attribute],
    field: &Ident,
    attr_name: &str,
) -> Result<Option<PropertyAttr>> {
    let Some(attr) = optional_attr(attrs, attr_name) else {
        return Ok(None);
    };
    parse_property_attr_body(attr, attrs, field).map(Some)
}

fn parse_property_attr_body(
    attr: &Attribute,
    attrs: &[Attribute],
    field: &Ident,
) -> Result<PropertyAttr> {
    let mut id = None;
    let mut name = None;
    let mut kind = None;
    let mut enum_type = None;
    let mut constraints = PropertyConstraintsAttr::default();

    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("id") {
            id = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("name") {
            name = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("kind") {
            kind = Some(ident_value(&meta)?);
        } else if meta.path.is_ident("enum_type") {
            enum_type = Some(meta.value()?.parse()?);
        } else if !constraints.parse_key(&meta)? {
            return Err(meta.error("unknown param attribute key"));
        }
        Ok(())
    })?;

    Ok(PropertyAttr {
        field: field.clone(),
        id: id.unwrap_or_else(|| LitStr::new(&field.to_string(), field.span())),
        name: name
            .unwrap_or_else(|| LitStr::new(&title_from_ident(&field.to_string()), field.span())),
        kind: required_value(kind, attr, "kind")?,
        enum_type,
        constraints,
        docs: doc_lines(attrs),
    })
}

fn property_def_tokens(property: &PropertyAttr) -> proc_macro2::TokenStream {
    let enum_def = match &property.enum_type {
        Some(enum_type) => quote!(Some(<#enum_type as ::lumen_engine::node::NodeEnum>::enum_def())),
        None => quote!(None),
    };
    PropertyDefTokens {
        id: &property.id,
        name: &property.name,
        kind: property_kind_tokens(&property.kind),
        enum_def,
        docs: &property.docs,
        constraints: &property.constraints,
        // `#[property]` fields hold a raw `PropertyExpression` from `parse_property`.
        accepts_expressions: quote!(true),
    }
    .tokens()
}

fn parse_enum_option_attr(
    attrs: &[Attribute],
    variant: &Ident,
    value: i64,
) -> Result<EnumOptionAttr> {
    let mut name = None;
    let mut label = None;

    if let Some(attr) = optional_attr(attrs, "enum_option") {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                name = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("label") {
                label = Some(meta.value()?.parse::<LitStr>()?.value());
            } else {
                return Err(meta.error("unknown enum_option attribute key"));
            }
            Ok(())
        })?;
    }

    let variant_name = variant.to_string();
    Ok(EnumOptionAttr {
        name: name.unwrap_or_else(|| to_snake_case(&variant_name)),
        label: label.unwrap_or_else(|| title_from_ident(&variant_name)),
        value,
    })
}

fn int_expr_value(expr: &Expr) -> Result<i64> {
    match expr {
        Expr::Lit(ExprLit {
            lit: syn::Lit::Int(lit),
            ..
        }) => parse_lit_int(lit),
        _ => Err(syn::Error::new(
            expr.span(),
            "NodeEnum variant discriminants must be integer literals",
        )),
    }
}

fn parse_lit_int(lit: &LitInt) -> Result<i64> {
    lit.base10_parse::<i64>()
}

fn required_attr<'a>(attrs: &'a [Attribute], name: &str) -> Result<&'a Attribute> {
    optional_attr(attrs, name)
        .ok_or_else(|| syn::Error::new(Span::call_site(), format!("missing #[{name}(...)]")))
}

fn has_attr(attrs: &[Attribute], name: &str) -> bool {
    optional_attr(attrs, name).is_some()
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

fn category_tokens(category: &Ident) -> proc_macro2::TokenStream {
    match category.to_string().as_str() {
        "compositing" | "Compositing" => quote!(::lumen_engine::node::NodeCategory::Compositing),
        "processing" | "Processing" => quote!(::lumen_engine::node::NodeCategory::Processing),
        "source" | "Source" => quote!(::lumen_engine::node::NodeCategory::Source),
        "output" | "Output" => quote!(::lumen_engine::node::NodeCategory::Output),
        "vector" | "Vector" => quote!(::lumen_engine::node::NodeCategory::Vector),
        _ => {
            let span = category.span();
            quote::quote_spanned!(span=> compile_error!("unknown node category"))
        }
    }
}

fn port_kind_tokens(kind: &Ident) -> proc_macro2::TokenStream {
    match kind.to_string().as_str() {
        "raster" | "Raster" => quote!(::lumen_engine::node::PortKind::Raster),
        "vector" | "Vector" => quote!(::lumen_engine::node::PortKind::Vector),
        _ => {
            let span = kind.span();
            quote::quote_spanned!(span=> compile_error!("unknown port kind"))
        }
    }
}
