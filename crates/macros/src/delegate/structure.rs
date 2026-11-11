use super::*;

fn is_vec_type(ty: &Type) -> bool {
    if let Type::Path(path) = ty {
        path.path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "Vec")
    } else {
        false
    }
}

/// The delegate type of a flattened field.
fn group_tokens(meta: &DelegateField) -> Option<proc_macro2::TokenStream> {
    let ty = &meta.ty;
    meta.property
        .flatten
        .then(|| quote!(<#ty as ::lumen_engine::node::Delegated>::Delegate))
}

/// The `MediaParam` of the field `meta` if it declares `#[media(...)]`, with its selector resolved
/// to that field's param id.
fn media_param_tokens(
    metas: &[DelegateField],
    meta: &DelegateField,
) -> Result<Option<proc_macro2::TokenStream>> {
    let Some(media) = &meta.property.media else {
        return Ok(None);
    };
    let selector = match &media.by {
        Some(by) => {
            let selector = metas
                .iter()
                .find(|other| other.ident == *by && !other.property.flatten)
                .ok_or_else(|| syn::Error::new(by.span(), "`by` names no param of this struct"))?;
            Some(&selector.property.id)
        }
        None => None,
    };
    Ok(Some(media.tokens(&meta.property.id, selector)))
}

pub(crate) fn expand_struct_delegate(
    input: &DeriveInput,
    fields: &Fields,
) -> Result<proc_macro2::TokenStream> {
    let ident = &input.ident;
    let delegate = parse_delegate_attr(input)?;
    let delegate_ident = delegate.name.clone();
    let fields = named_fields(fields)?;
    let metas = fields
        .iter()
        .map(|field| {
            let ident = field
                .ident
                .clone()
                .ok_or_else(|| syn::Error::new(field.span(), "Delegate requires named fields"))?;
            let property = parse_meta_attr(&field.attrs, &ident)?;
            Ok(DelegateField {
                ident,
                ty: field.ty.clone(),
                property,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let media_params = metas
        .iter()
        .map(|meta| media_param_tokens(&metas, meta))
        .collect::<Result<Vec<_>>>()?;

    let delegate_fields = metas.iter().map(|meta| {
        let ident = &meta.ident;
        let ty = &meta.ty;
        quote!(pub #ident: <#ty as ::lumen_engine::node::Delegated>::Delegate)
    });
    let default_fields = metas
        .iter()
        .map(|meta| {
            let ident = &meta.ident;
            quote!(#ident: ::lumen_engine::node::Delegated::into_delegate(evaluated.#ident))
        })
        .collect::<Vec<_>>();
    let nested_evaluated_fields = metas
        .iter()
        .map(|meta| {
            let ident = &meta.ident;
            let name = &meta.property.id;
            if meta.property.flatten {
                quote!(#ident: ::lumen_engine::node::DelegateEvaluable::eval(&self.#ident, ctx)?)
            } else {
                quote!(#ident: ctx.eval_field(#name, &self.#ident)?)
            }
        })
        .collect::<Vec<_>>();
    let property_defs = metas.iter().zip(&media_params).map(|(meta, media)| {
        match group_tokens(meta) {
            Some(group) => {
                let (ids, docs): (Vec<_>, Vec<_>) = meta
                    .property
                    .doc_overrides
                    .iter()
                    .map(|(id, doc)| (id, doc))
                    .unzip();
                quote! {
                    #[allow(unused_mut)]
                    let mut group = <#group as ::lumen_engine::node::NodeParams>::property_defs();
                    #(assert!(
                        <#group as ::lumen_engine::node::NodeParams>::is_property(#ids),
                        "`doc` names `{}`, which is not a param of the group",
                        #ids,
                    );)*
                    #[cfg(feature = "metadata")]
                    for def in &mut group {
                        match def.id {
                            #(#ids => def.description = #docs,)*
                            _ => {}
                        }
                    }
                    defs.extend(group);
                }
            }
            None => {
                let media = media
                    .as_ref()
                    .map_or_else(|| quote!(None), |media| quote!(Some(#media)));
                let def = property_def_tokens(&meta.property, &meta.ty, media);
                quote!(defs.push(#def);)
            }
        }
    });
    let is_property_checks = metas.iter().map(|meta| match group_tokens(meta) {
        Some(group) => quote!(<#group as ::lumen_engine::node::NodeParams>::is_property(id)),
        None => {
            let name = &meta.property.id;
            quote!(id == #name)
        }
    });
    let json_properties = metas.iter().map(|meta| {
        let ident = &meta.ident;
        let name = &meta.property.id;
        if meta.property.flatten {
            quote!(properties.extend(::lumen_engine::node::NodeParams::json_properties(&self.#ident));)
        } else {
            quote! {
                if let Some(value) = ::lumen_engine::node::DelegateEvaluable::to_json_value(&self.#ident) {
                    properties.push((#name, value));
                }
            }
        }
    });
    let property_matches = metas
        .iter()
        .filter(|meta| !meta.property.flatten)
        .map(|meta| {
            let ident = &meta.ident;
            let name = &meta.property.id;
            quote!(#name => ::lumen_engine::node::DelegateEvaluable::read(&self.#ident))
        });
    let group_property_reads = metas
        .iter()
        .filter(|meta| meta.property.flatten)
        .map(|meta| {
            let ident = &meta.ident;
            quote!(.or_else(|| ::lumen_engine::node::NodeParams::get_property(&self.#ident, id)))
        });
    let json_sets = metas.iter().map(|meta| {
        let ident = &meta.ident;
        let name = &meta.property.id;
        if meta.property.flatten {
            return quote!(::lumen_engine::node::NodeParams::set_json_fields(&mut self.#ident, fields, path, errors););
        }
        let parsed = if meta.property.constant {
            quote! {
                ::lumen_engine::json::reject_expression(value, &path)
                    .and_then(|()| ::lumen_engine::node::DelegateEvaluable::from_json_value(value, &path))
            }
        } else {
            quote!(::lumen_engine::node::DelegateEvaluable::from_json_value(value, &path))
        };
        let parsed = if meta.property.solid {
            quote!(#parsed.and_then(|value| ::lumen_engine::node::vector::paint::require_solid(value, &path)))
        } else {
            parsed
        };
        quote! {
            if let Some(value) = fields.get(#name) {
                let path = ::lumen_engine::node::field_path(path, #name);
                match #parsed {
                    Ok(value) => self.#ident = value,
                    Err(error) => errors.push((#name, error)),
                }
            }
        }
    });
    let media_sources = metas.iter().zip(&media_params).map(|(meta, media)| {
        let ident = &meta.ident;
        if meta.property.flatten {
            return quote!(::lumen_engine::node::NodeParams::for_each_media_source(&self.#ident, f););
        }
        let Some(media) = media else {
            return quote!();
        };
        let selected = match meta.property.media.as_ref().and_then(|media| media.by.as_ref()) {
            Some(by) => quote!(::lumen_engine::node::selected_media_kind(&self.#by, param.kinds)),
            None => quote!(None),
        };
        quote! {
            let param = #media;
            f(::lumen_engine::node::MediaSource::new(&param, &self.#ident, #selected));
        }
    });
    let field_ids = metas.iter().map(|meta| match group_tokens(meta) {
        Some(group) => {
            quote!(ids.extend(<#group as ::lumen_engine::node::NodeParams>::field_ids());)
        }
        None => {
            let name = &meta.property.id;
            quote!(ids.push(#name);)
        }
    });
    let required_ids = metas.iter().filter_map(|meta| match group_tokens(meta) {
        Some(group) => {
            Some(quote!(ids.extend(<#group as ::lumen_engine::node::NodeParams>::required_ids());))
        }
        None => {
            let name = &meta.property.id;
            meta.property.required.then(|| quote!(ids.push(#name);))
        }
    });
    let assert_unique = metas.iter().any(|meta| meta.property.flatten).then(
        || quote!(::lumen_engine::node::assert_unique_params(&defs, ::core::stringify!(#ident));),
    );
    let has_vec_fields = metas.iter().map(|meta| match group_tokens(meta) {
        Some(group) => quote!(<#group as ::lumen_engine::node::NodeParams>::HAS_VEC_FIELDS),
        None => {
            let has_vec = is_vec_type(&meta.ty);
            quote!(#has_vec)
        }
    });
    let field_idents = metas.iter().map(|meta| &meta.ident).collect::<Vec<_>>();
    let assert_field_types = metas
        .iter()
        .map(|meta| {
            let ty = &meta.ty;
            quote!(__lumen_delegate_assert_delegated::<#ty>();)
        })
        .collect::<Vec<_>>();

    Ok(quote! {
        #[derive(Debug, Clone, ::lumen_engine::fingerprint::StructuralHash)]
        pub struct #delegate_ident {
            #(#delegate_fields,)*
        }

        impl ::core::default::Default for #delegate_ident {
            fn default() -> Self {
                let evaluated = <#ident as ::core::default::Default>::default();
                Self {
                    #(#default_fields,)*
                }
            }
        }

        impl ::lumen_engine::node::NodeParams for #delegate_ident {
            const HAS_VEC_FIELDS: bool = false #(|| #has_vec_fields)*;

            fn property_defs() -> ::std::vec::Vec<::lumen_engine::node::PropertyDef> {
                let mut defs = ::std::vec::Vec::new();
                #(#property_defs)*
                #assert_unique
                defs
            }

            fn is_property(id: &str) -> bool {
                false #(|| #is_property_checks)*
            }

            fn json_properties(&self) -> ::std::vec::Vec<(&'static str, ::serde_json::Value)> {
                let mut properties = ::std::vec::Vec::new();
                #(#json_properties)*
                properties
            }

            fn get_property(&self, id: &str) -> ::core::option::Option<::lumen_engine::node::PropertyRead<'_>> {
                match id {
                    #(#property_matches,)*
                    _ => None #(#group_property_reads)*,
                }
            }

            fn for_each_media_source<'a>(&'a self, f: &mut dyn FnMut(::lumen_engine::node::MediaSource<'a>)) {
                #(#media_sources)*
            }

            fn field_ids() -> ::std::vec::Vec<&'static str> {
                let mut ids = ::std::vec::Vec::new();
                #(#field_ids)*
                ids
            }

            fn required_ids() -> ::std::vec::Vec<&'static str> {
                #[allow(unused_mut)]
                let mut ids = ::std::vec::Vec::new();
                #(#required_ids)*
                ids
            }

            fn set_json_fields(
                &mut self,
                fields: &::serde_json::Map<::std::string::String, ::serde_json::Value>,
                path: &str,
                errors: &mut ::std::vec::Vec<(&'static str, ::std::string::String)>,
            ) {
                #(#json_sets)*
            }
        }

        impl ::lumen_engine::node::Delegated for #ident {
            type Delegate = #delegate_ident;

            fn into_delegate(self) -> Self::Delegate {
                let evaluated = self;
                #delegate_ident {
                    #(#default_fields,)*
                }
            }
        }

        impl ::lumen_engine::node::DelegateEvaluable for #delegate_ident {
            type Evaluated = #ident;

            const ACCEPTS_EXPRESSIONS: bool = false;

            fn eval(
                &self,
                ctx: &::lumen_engine::node::DelegateEvalContext<'_>,
            ) -> ::core::result::Result<Self::Evaluated, ::lumen_engine::error::LumenError> {
                Ok(#ident {
                    #(#nested_evaluated_fields,)*
                })
            }

            fn for_each_expression(&self, f: &mut dyn FnMut(&::lumen_engine::expr::Expression)) {
                #(::lumen_engine::node::DelegateEvaluable::for_each_expression(&self.#field_idents, f);)*
            }

            fn from_json_value(
                value: &::serde_json::Value,
                path: &str,
            ) -> ::core::result::Result<Self, ::std::string::String> {
                let fields = ::lumen_engine::node::nested_fields(
                    value,
                    path,
                    &<Self as ::lumen_engine::node::NodeParams>::field_ids(),
                    &<Self as ::lumen_engine::node::NodeParams>::required_ids(),
                )?;
                <Self as ::lumen_engine::node::NodeParams>::from_json_fields(fields, path)
            }

            fn to_json_value(&self) -> ::core::option::Option<::serde_json::Value> {
                let properties = <Self as ::lumen_engine::node::NodeParams>::json_properties(self);
                Some(::serde_json::Value::Object(
                    properties
                        .into_iter()
                        .map(|(name, value)| (name.to_string(), value))
                        .collect(),
                ))
            }

            fn read(&self) -> ::core::option::Option<::lumen_engine::node::PropertyRead<'_>> {
                Some(::lumen_engine::node::PropertyRead::Value(None))
            }
        }

        const _: fn() = || {
            fn __lumen_delegate_assert_delegated<T: ::lumen_engine::node::Delegated>() {}
            #(#assert_field_types)*
        };
    })
}
