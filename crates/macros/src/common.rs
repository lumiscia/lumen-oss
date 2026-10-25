//! Attribute parsing and token helpers shared by the schema and delegate macros.

use quote::quote;
use syn::{Attribute, Ident, LitBool, LitFloat, LitInt, LitStr, Result, Token};

#[derive(Default)]
pub(crate) struct PropertyConstraintsAttr {
    min: Option<f64>,
    max: Option<f64>,
    step: Option<f64>,
    format: Option<LitStr>,
    multiline: bool,
    recommended_rows: Option<u32>,
    role: Option<LitStr>,
}

impl PropertyConstraintsAttr {
    /// Parses a constraint key, returning `false` when `meta` is not a constraint.
    pub(crate) fn parse_key(&mut self, meta: &syn::meta::ParseNestedMeta<'_>) -> Result<bool> {
        if meta.path.is_ident("min") {
            self.min = Some(float_value(meta)?);
        } else if meta.path.is_ident("max") {
            self.max = Some(float_value(meta)?);
        } else if meta.path.is_ident("step") {
            self.step = Some(float_value(meta)?);
        } else if meta.path.is_ident("format") {
            self.format = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("multiline") {
            self.multiline = optional_bool(meta)?;
        } else if meta.path.is_ident("recommended_rows") {
            self.recommended_rows = Some(meta.value()?.parse::<LitInt>()?.base10_parse()?);
        } else if meta.path.is_ident("role") {
            self.role = Some(meta.value()?.parse()?);
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    fn tokens(&self) -> proc_macro2::TokenStream {
        let min = option_f64_tokens(self.min);
        let max = option_f64_tokens(self.max);
        let step = option_f64_tokens(self.step);
        let format = option_lit_str_tokens(&self.format);
        let multiline = self.multiline;
        let recommended_rows = match self.recommended_rows {
            Some(value) => quote!(Some(#value)),
            None => quote!(None),
        };
        let role = option_lit_str_tokens(&self.role);
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
}

pub(crate) struct PropertyDefTokens<'a> {
    pub id: &'a LitStr,
    pub name: &'a LitStr,
    pub kind: proc_macro2::TokenStream,
    pub enum_def: proc_macro2::TokenStream,
    pub docs: &'a [String],
    pub constraints: &'a PropertyConstraintsAttr,
    pub accepts_expressions: proc_macro2::TokenStream,
}

impl PropertyDefTokens<'_> {
    pub(crate) fn tokens(&self) -> proc_macro2::TokenStream {
        let Self {
            id,
            name,
            kind,
            enum_def,
            accepts_expressions,
            ..
        } = self;
        let description = doc_string(self.docs);
        let constraints = self.constraints.tokens();
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
                #[cfg(feature = "metadata")]
                accepts_expressions: #accepts_expressions,
            }
        }
    }
}

pub(crate) fn property_kind_tokens(kind: &Ident) -> proc_macro2::TokenStream {
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

pub(crate) fn doc_lines(attrs: &[Attribute]) -> Vec<String> {
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

pub(crate) fn doc_string(lines: &[String]) -> String {
    lines.join("\n")
}

pub(crate) fn to_snake_case(value: &str) -> String {
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

pub(crate) fn title_from_ident(value: &str) -> String {
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

pub(crate) fn optional_attr<'a>(attrs: &'a [Attribute], name: &str) -> Option<&'a Attribute> {
    attrs.iter().find(|attr| attr.path().is_ident(name))
}

pub(crate) fn ident_value(meta: &syn::meta::ParseNestedMeta<'_>) -> Result<Ident> {
    let value = meta.value()?;
    if value.peek(LitStr) {
        let lit: LitStr = value.parse()?;
        Ok(Ident::new(&lit.value(), lit.span()))
    } else {
        value.parse()
    }
}

pub(crate) fn optional_bool(meta: &syn::meta::ParseNestedMeta<'_>) -> Result<bool> {
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
