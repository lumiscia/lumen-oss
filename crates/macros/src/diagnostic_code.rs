use quote::quote;
use syn::{Data, DeriveInput, Fields, Ident, LitStr, Result, spanned::Spanned};

enum VariantCode {
    Code(LitStr),
    /// Takes the code of the variant's only field, or of the named field.
    Transparent(Option<Ident>),
}

fn variant_code(variant: &syn::Variant) -> Result<VariantCode> {
    let mut found = None;
    for attr in variant
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("code"))
    {
        if found.is_some() {
            return Err(syn::Error::new(
                attr.span(),
                "only one `#[code]` per variant",
            ));
        }
        if let Ok(code) = attr.parse_args::<LitStr>() {
            found = Some(VariantCode::Code(code));
            continue;
        }
        let mut field = None;
        attr.parse_nested_meta(|meta| {
            if !meta.path.is_ident("transparent") {
                return Err(meta.error("expected a code string or `transparent`"));
            }
            if meta.input.peek(syn::Token![=]) {
                field = Some(meta.value()?.parse::<Ident>()?);
            }
            Ok(())
        })?;
        found = Some(VariantCode::Transparent(field));
    }
    found.ok_or_else(|| {
        syn::Error::new(
            variant.span(),
            "every variant needs `#[code(\"area.problem\")]` or `#[code(transparent)]`",
        )
    })
}

fn valid_code(code: &str) -> bool {
    let mut parts = code.split('.');
    let segment = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|char| char.is_ascii_lowercase() || char.is_ascii_digit() || char == '_')
    };
    parts.clone().count() >= 2 && parts.all(segment)
}

pub(crate) fn expand(input: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    let ident = &input.ident;
    let Data::Enum(data) = &input.data else {
        return Err(syn::Error::new(
            input.span(),
            "DiagnosticCode derives only on enums",
        ));
    };
    let mut codes: Vec<LitStr> = Vec::new();
    let mut arms = Vec::new();
    for variant in &data.variants {
        let name = &variant.ident;
        match variant_code(variant)? {
            VariantCode::Code(code) => {
                if !valid_code(&code.value()) {
                    return Err(syn::Error::new(
                        code.span(),
                        "codes are dot-separated lowercase segments, such as `graph.cycle`",
                    ));
                }
                if codes.iter().any(|other| other.value() == code.value()) {
                    return Err(syn::Error::new(code.span(), "this code is already used"));
                }
                let pattern = match &variant.fields {
                    Fields::Unit => quote!(Self::#name),
                    Fields::Named(_) => quote!(Self::#name { .. }),
                    Fields::Unnamed(_) => quote!(Self::#name(..)),
                };
                arms.push(quote!(#pattern => #code));
                codes.push(code);
            }
            VariantCode::Transparent(field) => {
                let arm = match (&variant.fields, field) {
                    (Fields::Unnamed(fields), None) if fields.unnamed.len() == 1 => {
                        quote!(Self::#name(inner) => inner.code())
                    }
                    (Fields::Named(_), Some(field)) => {
                        quote!(Self::#name { #field, .. } => #field.code())
                    }
                    _ => {
                        return Err(syn::Error::new(
                            variant.span(),
                            "`transparent` takes a variant with one unnamed field, or names a field as `transparent = field`",
                        ));
                    }
                };
                arms.push(arm);
            }
        }
    }
    Ok(quote! {
        impl #ident {
            /// The codes this type's own variants have. Transparent variants take theirs from what
            /// they wrap.
            pub const CODES: &'static [&'static str] = &[#(#codes),*];

            /// The stable, public code of this error.
            pub fn code(&self) -> &'static str {
                match self {
                    #(#arms,)*
                }
            }
        }
    })
}
