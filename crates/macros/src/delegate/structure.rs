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
    let nested_evaluated_fields = metas
        .iter()
        .map(|meta| {
            let ident = &meta.ident;
            let name = &meta.property.id;
            quote!(#ident: ctx.eval_field(#name, &self.#ident)?)
        })
        .collect::<Vec<_>>();
    let property_defs = metas
        .iter()
        .map(|meta| property_def_tokens(&meta.property, &meta.ty));
    let is_property_matches = metas.iter().map(|meta| {
        let name = &meta.property.id;
        quote!(#name)
    });
    let json_properties = metas.iter().map(|meta| {
        let ident = &meta.ident;
        let name = &meta.property.id;
        quote! {
            if let Some(value) = ::lumen_engine::node::DelegateEvaluable::to_json_value(&self.#ident) {
                properties.push((#name, value));
            }
        }
    });
    let property_matches = metas.iter().map(|meta| {
        let ident = &meta.ident;
        let name = &meta.property.id;
        quote!(#name => ::lumen_engine::node::DelegateEvaluable::read(&self.#ident))
    });
    let json_sets = metas.iter().map(|meta| {
        let ident = &meta.ident;
        let name = &meta.property.id;
        let reject_expression = meta.property.constant.then(|| {
            quote!(::lumen_engine::json::reject_expression(value, &path)?;)
        });
        quote! {
            if let Some(value) = fields.get(#name) {
                let path = ::lumen_engine::node::field_path(path, #name);
                #reject_expression
                delegate.#ident = ::lumen_engine::node::DelegateEvaluable::from_json_value(value, &path)?;
            }
        }
    });
    let field_ids = metas.iter().map(|meta| &meta.property.id);
    let required_ids = metas
        .iter()
        .filter(|meta| meta.property.required)
        .map(|meta| &meta.property.id);
    let has_vec_fields = metas.iter().any(|meta| is_vec_type(&meta.ty));
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
            const HAS_VEC_FIELDS: bool = #has_vec_fields;

            fn property_defs() -> ::std::vec::Vec<::lumen_engine::node::PropertyDef> {
                vec![#(#property_defs),*]
            }

            fn is_property(id: &str) -> bool {
                matches!(id, #(#is_property_matches)|*)
            }

            fn json_properties(&self) -> ::std::vec::Vec<(&'static str, ::serde_json::Value)> {
                let mut properties = ::std::vec::Vec::new();
                #(#json_properties)*
                properties
            }

            fn get_property(&self, id: &str) -> ::core::option::Option<::lumen_engine::node::PropertyRead<'_>> {
                match id {
                    #(#property_matches,)*
                    _ => None,
                }
            }

            fn from_json_fields(
                fields: &::serde_json::Map<::std::string::String, ::serde_json::Value>,
                path: &str,
            ) -> ::core::result::Result<Self, ::std::string::String> {
                let mut delegate = <Self as ::core::default::Default>::default();
                #(#json_sets)*
                Ok(delegate)
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

            fn has_expressions(&self) -> bool {
                false #(|| ::lumen_engine::node::DelegateEvaluable::has_expressions(&self.#field_idents))*
            }

            fn from_json_value(
                value: &::serde_json::Value,
                path: &str,
            ) -> ::core::result::Result<Self, ::std::string::String> {
                let fields = ::lumen_engine::node::nested_fields(
                    value,
                    path,
                    &[#(#field_ids),*],
                    &[#(#required_ids),*],
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
