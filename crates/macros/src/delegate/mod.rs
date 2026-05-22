use quote::{format_ident, quote};
use syn::{
    Attribute, Data, DeriveInput, Fields, Ident, LitBool, LitFloat, LitInt, LitStr, Path, Result,
    Token, Type, Variant, punctuated::Punctuated, spanned::Spanned,
};

pub(crate) fn expand_delegate(input: DeriveInput) -> Result<proc_macro2::TokenStream> {
    match &input.data {
        Data::Struct(data) => structure::expand_struct_delegate(&input, &data.fields),
        Data::Enum(data) => enums::expand_enum_delegate(&input, &data.variants),
        Data::Union(_) => Err(syn::Error::new(
            input.ident.span(),
            "Delegate derive does not support unions",
        )),
    }
}

mod enums;
mod structure;

struct DelegateField {
    ident: Ident,
    ty: Type,
    property: PropertyAttr,
}

struct PropertyAttr {
    id: LitStr,
    name: LitStr,
    kind: Option<Ident>,
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

struct DelegateAttr {
    name: Ident,
    kind: Option<Ident>,
}

fn parse_delegate_attr(input: &DeriveInput) -> Result<DelegateAttr> {
    let mut name = None;
    let mut kind = None;
    if let Some(attr) = optional_attr(&input.attrs, "delegate") {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                name = Some(meta.value()?.parse::<Ident>()?);
            } else if meta.path.is_ident("kind") {
                kind = Some(ident_value(&meta)?);
            } else {
                return Err(meta.error("unknown delegate attribute key"));
            }
            Ok(())
        })?;
    }
    Ok(DelegateAttr {
        name: name.unwrap_or_else(|| format_ident!("{}Delegate", input.ident)),
        kind,
    })
}

fn parse_meta_attr(attrs: &[Attribute], field: &Ident) -> Result<PropertyAttr> {
    let attr = optional_attr(attrs, "meta")
        .ok_or_else(|| syn::Error::new(field.span(), "Delegate fields require #[meta]"))?;
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
            return Err(meta.error("unknown meta attribute key"));
        }
        Ok(())
    })?;

    Ok(PropertyAttr {
        id: id.unwrap_or_else(|| LitStr::new(&field.to_string(), field.span())),
        name: name
            .unwrap_or_else(|| LitStr::new(&title_from_ident(&field.to_string()), field.span())),
        kind,
        enum_type,
        constraints,
        docs: doc_lines(attrs),
    })
}

fn property_def_tokens(property: &PropertyAttr, ty: &Type) -> proc_macro2::TokenStream {
    let id = &property.id;
    let name = &property.name;
    let kind = match &property.kind {
        Some(kind) => property_kind_tokens(kind),
        None => quote!(<#ty as ::lumen_engine::node::NodeParamType>::property_kind()),
    };
    let enum_def = match &property.enum_type {
        Some(enum_type) => quote!(Some(<#enum_type as ::lumen_engine::node::NodeEnum>::enum_def())),
        None => quote!(<#ty as ::lumen_engine::node::NodeParamType>::enum_def()),
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

fn named_fields(fields: &Fields) -> Result<&Punctuated<syn::Field, Token![,]>> {
    match fields {
        Fields::Named(fields) => Ok(&fields.named),
        _ => Err(syn::Error::new(
            fields.span(),
            "Delegate derive requires named struct fields",
        )),
    }
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

fn optional_attr<'a>(attrs: &'a [Attribute], name: &str) -> Option<&'a Attribute> {
    attrs.iter().find(|attr| attr.path().is_ident(name))
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
