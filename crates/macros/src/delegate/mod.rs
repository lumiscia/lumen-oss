use quote::{format_ident, quote};
use syn::{
    Attribute, Data, DeriveInput, Fields, Ident, LitStr, Path, Result, Token, Type, Variant,
    punctuated::Punctuated, spanned::Spanned,
};

use crate::common::{
    PropertyConstraintsAttr, PropertyDefTokens, doc_lines, ident_value, optional_attr,
    optional_bool, property_kind_tokens, title_from_ident,
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
    constant: bool,
    docs: Vec<String>,
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
    let mut constant = false;
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
        } else if meta.path.is_ident("constant") {
            constant = optional_bool(&meta)?;
        } else if !constraints.parse_key(&meta)? {
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
        constant,
        docs: doc_lines(attrs),
    })
}

fn property_def_tokens(property: &PropertyAttr, ty: &Type) -> proc_macro2::TokenStream {
    let kind = match &property.kind {
        Some(kind) => property_kind_tokens(kind),
        None => quote!(<#ty as ::lumen_engine::node::NodeParamType>::property_kind()),
    };
    let enum_def = match &property.enum_type {
        Some(enum_type) => quote!(Some(<#enum_type as ::lumen_engine::node::NodeEnum>::enum_def())),
        None => quote!(<#ty as ::lumen_engine::node::NodeParamType>::enum_def()),
    };
    // `Vec` fields are deserialized whole by serde, so an expression string never parses.
    // Constant fields reject expressions at parse time.
    let accepts_expressions = if property.constant || structure::is_vec_type(ty) {
        quote!(false)
    } else {
        quote!(<<#ty as ::lumen_engine::node::Delegated>::Delegate as ::lumen_engine::node::DelegateValue>::ACCEPTS_EXPRESSIONS)
    };
    PropertyDefTokens {
        id: &property.id,
        name: &property.name,
        kind,
        enum_def,
        docs: &property.docs,
        constraints: &property.constraints,
        accepts_expressions,
    }
    .tokens()
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
