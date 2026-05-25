//! Node schema proc macros.
//!
//! `#[derive(Node)]` turns a renderer-neutral node struct into canonical
//! schema metadata plus property lookup glue. It intentionally does not model
//! UI concerns or multiple outputs.

use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::{
    Attribute, Data, DeriveInput, Expr, ExprLit, Fields, Ident, LitBool, LitFloat, LitInt, LitStr,
    Path, Result, Token, punctuated::Punctuated, spanned::Spanned,
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

#[derive(Default)]
struct PropertyConstraintsAttr {
    min: Option<f64>,
    max: Option<f64>,
    step: Option<f64>,
    format: Option<LitStr>,
    multiline: bool,
    recommended_rows: Option<u32>,
    role: Option<LitStr>,
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
        } else if meta.path.is_ident("min") {
            constraints.min = Some(float_value(&meta)?);
        } else if meta.path.is_ident("max") {
            constraints.max = Some(float_value(&meta)?);
        } else if meta.path.is_ident("step") {
            constraints.step = Some(float_value(&meta)?);
        } else if meta.path.is_ident("format") {
            constraints.format = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("multiline") {
            constraints.multiline = optional_bool(&meta)?;
        } else if meta.path.is_ident("recommended_rows") {
            constraints.recommended_rows = Some(meta.value()?.parse::<LitInt>()?.base10_parse()?);
        } else if meta.path.is_ident("role") {
            constraints.role = Some(meta.value()?.parse()?);
        } else {
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
    let id = &property.id;
    let name = &property.name;
    let kind = property_kind_tokens(&property.kind);
    let enum_def = match &property.enum_type {
        Some(enum_type) => quote!(Some(<#enum_type as ::lumen_engine::node::NodeEnum>::enum_def())),
        None => quote!(None),
    };
    let description = doc_string(&property.docs);
    let constraints = property_constraints_tokens(&property.constraints);
    quote! {
        ::lumen_engine::node::PropertyDef {
            id: #id,
            expected: #kind,
            #[cfg(any(feature = "json", feature = "metadata"))]
            enum_def: #enum_def,
            #[cfg(feature = "metadata")]
            name: #name,
            #[cfg(feature = "metadata")]
            description: #description,
            #[cfg(feature = "metadata")]
            constraints: #constraints,
        }
    }
}

fn property_constraints_tokens(constraints: &PropertyConstraintsAttr) -> proc_macro2::TokenStream {
    let min = option_f64_tokens(constraints.min);
    let max = option_f64_tokens(constraints.max);
    let step = option_f64_tokens(constraints.step);
    let format = option_lit_str_tokens(&constraints.format);
    let multiline = constraints.multiline;
    let recommended_rows = match constraints.recommended_rows {
        Some(value) => quote!(Some(#value)),
        None => quote!(None),
    };
    let role = option_lit_str_tokens(&constraints.role);
    quote! {
        ::lumen_engine::node::PropertyConstraints {
            min: #min,
            max: #max,
            step: #step,
            format: #format,
            multiline: #multiline,
            recommended_rows: #recommended_rows,
            role: #role,
        }
    }
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

fn to_snake_case(value: &str) -> String {
    let mut out = String::new();
    for (index, ch) in value.chars().enumerate() {
        if ch.is_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn title_from_ident(value: &str) -> String {
    let mut out = String::new();
    for (index, part) in to_snake_case(value).split('_').enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

fn doc_lines(attrs: &[Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .filter_map(|attr| match &attr.meta {
            syn::Meta::NameValue(name_value) => match &name_value.value {
                syn::Expr::Lit(expr_lit) => match &expr_lit.lit {
                    syn::Lit::Str(lit) => Some(lit.value().trim().to_string()),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn doc_string(lines: &[String]) -> String {
    lines.join("\n")
}

fn required_attr<'a>(attrs: &'a [Attribute], name: &str) -> Result<&'a Attribute> {
    optional_attr(attrs, name)
        .ok_or_else(|| syn::Error::new(Span::call_site(), format!("missing #[{name}(...)]")))
}

fn optional_attr<'a>(attrs: &'a [Attribute], name: &str) -> Option<&'a Attribute> {
    attrs.iter().find(|attr| attr.path().is_ident(name))
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

fn float_value(meta: &syn::meta::ParseNestedMeta<'_>) -> Result<f64> {
    let value = meta.value()?;
    if value.peek(LitFloat) {
        value.parse::<LitFloat>()?.base10_parse()
    } else {
        value
            .parse::<LitInt>()?
            .base10_parse::<i64>()
            .map(|value| value as f64)
    }
}

fn option_f64_tokens(value: Option<f64>) -> proc_macro2::TokenStream {
    match value {
        Some(value) => quote!(Some(#value)),
        None => quote!(None),
    }
}

fn option_lit_str_tokens(value: &Option<LitStr>) -> proc_macro2::TokenStream {
    match value {
        Some(value) => quote!(Some(#value)),
        None => quote!(None),
    }
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

fn property_kind_tokens(kind: &Ident) -> proc_macro2::TokenStream {
    match kind.to_string().as_str() {
        "float" | "Float" => quote!(::lumen_engine::node::PropertyKind::Float),
        "int" | "Int" => quote!(::lumen_engine::node::PropertyKind::Int),
        "bool" | "Bool" => quote!(::lumen_engine::node::PropertyKind::Bool),
        "string" | "String" => quote!(::lumen_engine::node::PropertyKind::String),
        "color" | "Color" => quote!(::lumen_engine::node::PropertyKind::Color),
        "vec2" | "Vec2" => quote!(::lumen_engine::node::PropertyKind::Vec2),
        "enum" | "Enum" => quote!(::lumen_engine::node::PropertyKind::Enum),
        "paint" | "Paint" => quote!(::lumen_engine::node::PropertyKind::Paint),
        _ => {
            let span = kind.span();
            quote::quote_spanned!(span=> compile_error!("unknown param kind"))
        }
    }
}
