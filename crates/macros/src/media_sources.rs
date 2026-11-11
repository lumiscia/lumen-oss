use quote::quote;
use syn::{Data, DeriveInput, Fields, LitStr, Result};

use crate::common::MediaAttr;

/// `MEDIA_PARAMS` and `for_each_media_source` for a struct whose `String` fields carry
/// `#[media(...)]`, which `by` does not apply to.
pub(crate) fn expand(input: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    let ident = &input.ident;
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            ident,
            "MediaSources only supports structs",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            ident,
            "MediaSources requires named struct fields",
        ));
    };

    let mut params = Vec::new();
    let mut visits = Vec::new();
    for field in &fields.named {
        let Some(media) = MediaAttr::parse(&field.attrs)? else {
            continue;
        };
        if let Some(by) = &media.by {
            return Err(syn::Error::new(
                by.span(),
                "`by` only applies to node params",
            ));
        }
        let field_ident = field.ident.as_ref().expect("named field");
        let name = LitStr::new(&field_ident.to_string(), field_ident.span());
        let param = media.tokens(&name, None);
        visits.push(quote! {
            f(::lumen_engine::node::MediaSource::new(&#param, &self.#field_ident, None));
        });
        params.push(param);
    }

    Ok(quote! {
        impl #ident {
            /// The fields declared with `#[media(...)]`.
            pub const MEDIA_PARAMS: &'static [::lumen_engine::node::MediaParam] = &[#(#params),*];

            /// Calls `f` with each field declared with `#[media(...)]`.
            pub fn for_each_media_source<'a>(
                &'a self,
                f: &mut dyn FnMut(::lumen_engine::node::MediaSource<'a>),
            ) {
                #(#visits)*
            }
        }
    })
}
