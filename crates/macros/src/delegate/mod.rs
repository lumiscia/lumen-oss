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
    let structural_hash = crate::structural_hash::expand(&input)?;
    let delegate = match &input.data {
        Data::Struct(data) => structure::expand_struct_delegate(&input, &data.fields),
        Data::Enum(data) => enums::expand_enum_delegate(&input, &data.variants),
        Data::Union(_) => Err(syn::Error::new(
            input.ident.span(),
            "Delegate derive does not support unions",
        )),
    }?;
    Ok(quote!(#structural_hash #delegate))
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
    /// The field is a `Paint` that must be a solid color, so parsing rejects a gradient.
    solid: bool,
    /// The field must be set when parsing a `properties` struct's JSON.
    required: bool,
    docs: Vec<String>,
    /// The field's type is a group whose params are this struct's own, with no path segment.
    flatten: bool,
    /// Docs that a flattened group's params have in this struct, by param id.
    doc_overrides: Vec<(LitStr, LitStr)>,
}

struct DelegateAttr {
    name: Ident,
    kind: Option<Ident>,
    /// Adds an `Expr` variant evaluated through the enum's `ParamValue` impl.
    expressions: bool,
}

fn parse_delegate_attr(input: &DeriveInput) -> Result<DelegateAttr> {
    let mut name = None;
    let mut kind = None;
    let mut expressions = false;
    if let Some(attr) = optional_attr(&input.attrs, "delegate") {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                name = Some(meta.value()?.parse::<Ident>()?);
            } else if meta.path.is_ident("kind") {
                kind = Some(ident_value(&meta)?);
            } else if meta.path.is_ident("expressions") {
                expressions = true;
            } else {
                return Err(meta.error("unknown delegate attribute key"));
            }
            Ok(())
        })?;
    }
    Ok(DelegateAttr {
        name: name.unwrap_or_else(|| format_ident!("{}Delegate", input.ident)),
        kind,
        expressions,
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
    let mut solid = false;
    let mut required = false;
    let mut constraints = PropertyConstraintsAttr::default();
    let mut flatten = false;
    let mut doc_overrides = Vec::new();
    let mut param_keys = false;

    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("flatten") {
            flatten = true;
            return Ok(());
        }
        if meta.path.is_ident("doc") {
            return meta.parse_nested_meta(|param| {
                let id = param
                    .path
                    .get_ident()
                    .ok_or_else(|| param.error("expected a param id"))?;
                let id = LitStr::new(&id.to_string(), id.span());
                doc_overrides.push((id, param.value()?.parse()?));
                Ok(())
            });
        }
        param_keys = true;
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
        } else if meta.path.is_ident("solid") {
            solid = optional_bool(&meta)?;
        } else if meta.path.is_ident("required") {
            required = optional_bool(&meta)?;
        } else if !constraints.parse_key(&meta)? {
            return Err(meta.error("unknown meta attribute key"));
        }
        Ok(())
    })?;
    if flatten && param_keys {
        return Err(syn::Error::new_spanned(
            attr,
            "a `flatten` field's params are the group's own, so it only takes `doc(...)`",
        ));
    }
    if !flatten && !doc_overrides.is_empty() {
        return Err(syn::Error::new_spanned(
            attr,
            "`doc(...)` requires `flatten`",
        ));
    }

    Ok(PropertyAttr {
        id: id.unwrap_or_else(|| LitStr::new(&field.to_string(), field.span())),
        name: name
            .unwrap_or_else(|| LitStr::new(&title_from_ident(&field.to_string()), field.span())),
        kind: kind.or_else(|| solid.then(|| format_ident!("color"))),
        enum_type,
        constraints,
        constant,
        solid,
        required,
        docs: doc_lines(attrs),
        flatten,
        doc_overrides,
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
    let delegate = quote!(<#ty as ::lumen_engine::node::Delegated>::Delegate);
    // Constant fields reject expressions at parse time.
    let accepts_expressions = if property.constant {
        quote!(false)
    } else {
        quote!(<#delegate as ::lumen_engine::node::DelegateEvaluable>::ACCEPTS_EXPRESSIONS)
    };
    let nullable = quote!(<#delegate as ::lumen_engine::node::DelegateEvaluable>::NULLABLE);
    PropertyDefTokens {
        id: &property.id,
        name: &property.name,
        kind,
        enum_def,
        docs: &property.docs,
        constraints: &property.constraints,
        accepts_expressions,
        nullable,
        required: property.required,
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
