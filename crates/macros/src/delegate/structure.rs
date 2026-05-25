use super::*;

pub(crate) fn is_vec_type(ty: &Type) -> bool {
    if let Type::Path(path) = ty {
        path.path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "Vec")
    } else {
        false
    }
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
    let evaluated_fields = metas
        .iter()
        .map(|meta| {
            let ident = &meta.ident;
            let name = &meta.property.id;
            quote! {
                #ident: ::lumen_engine::node::DelegateEvaluable::eval(
                    &self.#ident,
                    &::lumen_engine::node::DelegateEvalContext {
                        node_id: ctx.node_id,
                        property_path: #name,
                        expr: ctx.expr,
                    },
                )?
            }
        })
        .collect::<Vec<_>>();
    let property_defs = metas
        .iter()
        .map(|meta| property_def_tokens(&meta.property, &meta.ty));
    let is_property_matches = metas.iter().map(|meta| {
        let name = &meta.property.id;
        quote!(#name)
    });
    let default_properties = metas.iter().map(|meta| {
        let ident = &meta.ident;
        let name = &meta.property.id;
        let ty = &meta.ty;
        if is_vec_type(ty) {
            quote!((#name, ::lumen_engine::node::PropertyValue::String(String::from("[]"))))
        } else {
            quote!((#name, ::lumen_engine::node::DelegateValue::to_property_value(&self.#ident)))
        }
    });
    let property_matches = metas.iter().map(|meta| {
        let ident = &meta.ident;
        let name = &meta.property.id;
        let ty = &meta.ty;
        if is_vec_type(ty) {
            quote!(#name => None)
        } else {
            quote!(#name => Some(::lumen_engine::node::DelegateValue::to_property_expression(&self.#ident)))
        }
    });
    let json_sets = metas.iter().map(|meta| {
        let ident = &meta.ident;
        let name = &meta.property.id;
        let ty = &meta.ty;
        if is_vec_type(ty) {
            quote! {
                if let Some(value) = params.and_then(|params| params.get(#name)) {
                    metas.#ident = ::serde_json::from_value(value.clone())?;
                }
            }
        } else {
            let property_def = property_def_tokens(&meta.property, ty);
            quote! {
                if let Some(value) = params.and_then(|params| params.get(#name)) {
                    let def = #property_def;
                    metas.#ident = ::lumen_engine::node::DelegateValue::from_property_expression(
                        ::lumen_engine::json::parse_property(value, Some(&def), #name)?,
                    )?;
                }
            }
        }
    });
    let assert_field_types = metas
        .iter()
        .map(|meta| {
            let ty = &meta.ty;
            quote!(__lumen_delegate_assert_delegated::<#ty>();)
        })
        .collect::<Vec<_>>();

    Ok(quote! {
        #[derive(Debug, Clone)]
        #[cfg_attr(feature = "json", derive(::serde::Deserialize), serde(default))]
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
            type Evaluated = #ident;

            fn property_defs() -> ::std::vec::Vec<::lumen_engine::node::PropertyDef> {
                vec![#(#property_defs),*]
            }

            fn is_property(id: &str) -> bool {
                matches!(id, #(#is_property_matches)|*)
            }

            fn default_properties(&self) -> ::std::vec::Vec<(&'static str, ::lumen_engine::node::PropertyValue)> {
                vec![#(#default_properties),*]
            }

            fn get_property(&self, id: &str) -> ::core::option::Option<::lumen_engine::node::PropertyExpression> {
                match id {
                    #(#property_matches,)*
                    _ => None,
                }
            }

            fn eval(
                &self,
                ctx: &::lumen_engine::node::NodeParamEvalContext<'_>,
            ) -> ::core::result::Result<Self::Evaluated, ::lumen_engine::error::LumenError> {
                Ok(#ident {
                    #(#evaluated_fields,)*
                })
            }

            #[cfg(feature = "json")]
            fn from_json(
                params: ::core::option::Option<&::serde_json::Map<::std::string::String, ::serde_json::Value>>,
            ) -> ::anyhow::Result<Self>
            where
                Self: Sized,
                Self: ::serde::de::DeserializeOwned,
            {
                let mut metas = <Self as ::core::default::Default>::default();
                #(#json_sets)*
                Ok(metas)
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

            fn eval(
                &self,
                ctx: &::lumen_engine::node::DelegateEvalContext<'_>,
            ) -> ::core::result::Result<Self::Evaluated, ::lumen_engine::error::LumenError> {
                Ok(#ident {
                    #(#evaluated_fields,)*
                })
            }
        }

        const _: fn() = || {
            fn __lumen_delegate_assert_delegated<T: ::lumen_engine::node::Delegated>() {}
            #(#assert_field_types)*
        };
    })
}
