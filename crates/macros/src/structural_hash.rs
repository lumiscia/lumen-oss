use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, Member, Result, parse_quote};

pub(crate) fn expand(input: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    let ident = &input.ident;
    let mut generics = input.generics.clone();
    let mut hash_fields = |fields: &Fields| -> Result<Vec<_>> {
        fields
            .iter()
            .enumerate()
            .filter_map(|(index, field)| {
                let mut skip = false;
                for attr in &field.attrs {
                    if attr.path().is_ident("structural_hash") {
                        if let Err(error) = attr.parse_nested_meta(|meta| {
                            if meta.path.is_ident("skip") {
                                skip = true;
                                Ok(())
                            } else {
                                Err(meta.error("expected `skip`"))
                            }
                        }) {
                            return Some(Err(error));
                        }
                    }
                }
                if skip {
                    return None;
                }
                let ty = &field.ty;
                if !input.generics.params.is_empty() {
                    generics
                        .make_where_clause()
                        .predicates
                        .push(parse_quote!(#ty: ::lumen_engine::fingerprint::StructuralHash));
                }
                Some(Ok(match &field.ident {
                    Some(ident) => Member::Named(ident.clone()),
                    None => Member::Unnamed(index.into()),
                }))
            })
            .collect()
    };
    let body = match &input.data {
        Data::Struct(data) => {
            let members = hash_fields(&data.fields)?;
            quote! {
                #(::lumen_engine::fingerprint::StructuralHash::structural_hash(&self.#members, state);)*
            }
        }
        Data::Enum(data) => {
            let arms = data.variants.iter().map(|variant| {
                let name = &variant.ident;
                let members = hash_fields(&variant.fields)?;
                let bindings = members.iter().enumerate()
                    .map(|(index, _)| format_ident!("field_{index}")).collect::<Vec<_>>();
                // Skipped fields are wildcards; every other field gets a binding.
                let pattern = match &variant.fields {
                    Fields::Unit => quote!(Self::#name),
                    Fields::Named(_) => quote!(Self::#name { #(#members: #bindings,)* .. }),
                    Fields::Unnamed(fields) => {
                        let patterns = (0..fields.unnamed.len()).map(|index| {
                            members.iter().position(|member| *member == Member::Unnamed(index.into()))
                                .map(|index| { let binding = &bindings[index]; quote!(#binding) })
                                .unwrap_or_else(|| quote!(_))
                        });
                        quote!(Self::#name(#(#patterns),*))
                    }
                };
                Ok(quote! {
                    #pattern => {
                        #(::lumen_engine::fingerprint::StructuralHash::structural_hash(#bindings, state);)*
                    }
                })
            }).collect::<Result<Vec<_>>>()?;
            quote! {
                ::core::hash::Hash::hash(&::core::mem::discriminant(self), state);
                match self { #(#arms,)* }
            }
        }
        Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                ident,
                "StructuralHash does not support unions",
            ));
        }
    };
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics ::lumen_engine::fingerprint::StructuralHash for #ident #ty_generics #where_clause {
            fn structural_hash<__LumenHasher: ::core::hash::Hasher>(&self, state: &mut __LumenHasher) {
                #body
            }
        }
    })
}
