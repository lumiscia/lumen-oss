use super::*;

fn is_type_named(ty: &Type, names: &[&str]) -> bool {
    if let Type::Path(path) = ty {
        path.path
            .segments
            .last()
            .is_some_and(|segment| names.iter().any(|name| segment.ident == name))
    } else {
        false
    }
}

pub(crate) fn is_vec_type(ty: &Type) -> bool {
    is_type_named(ty, &["Vec"])
}

/// `Option` and `Override` fields, whose delegates implement `OptionalDelegate`.
pub(crate) fn is_optional_type(ty: &Type) -> bool {
    is_type_named(ty, &["Option", "Override"])
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
    let default_properties = metas.iter().map(|meta| {
        let ident = &meta.ident;
        let name = &meta.property.id;
        let ty = &meta.ty;
        if is_vec_type(ty) {
            quote!()
        } else if is_optional_type(ty) {
            quote! {
                if let ::lumen_engine::node::Override::Set(value) =
                    ::lumen_engine::node::OptionalDelegate::as_override(&self.#ident)
                {
                    properties.push((#name, ::lumen_engine::node::DelegateValue::to_json_value(value)));
                }
            }
        } else {
            quote!(properties.push((#name, ::lumen_engine::node::DelegateValue::to_json_value(&self.#ident)));)
        }
    });
    let property_matches = metas.iter().map(|meta| {
        let ident = &meta.ident;
        let name = &meta.property.id;
        let ty = &meta.ty;
        if is_vec_type(ty) {
            quote!(#name => None)
        } else if is_optional_type(ty) {
            quote! {
                #name => match ::lumen_engine::node::OptionalDelegate::as_override(&self.#ident) {
                    ::lumen_engine::node::Override::Set(value) => {
                        Some(::lumen_engine::node::DelegateValue::read(value))
                    }
                    _ => None,
                }
            }
        } else {
            quote!(#name => Some(::lumen_engine::node::DelegateValue::read(&self.#ident)))
        }
    });
    let json_sets = metas.iter().map(|meta| {
        let ident = &meta.ident;
        let name = &meta.property.id;
        let ty = &meta.ty;
        if is_vec_type(ty) {
            return quote! {
                if let Some(value) = params.and_then(|params| params.get(#name)) {
                    metas.#ident = ::serde_json::from_value(value.clone()).map_err(|error| {
                        ::anyhow::anyhow!("`{}` on node {node_id}: {error}", #name)
                    })?;
                }
            };
        }
        let property_def = property_def_tokens(&meta.property, ty);
        let reject_expression = meta.property.constant.then(|| {
            quote! {
                if matches!(parsed, ::lumen_engine::node::PropertyExpression::Expr(_)) {
                    ::anyhow::bail!(
                        "`{}` on node {node_id} does not support expressions",
                        #name
                    );
                }
            }
        });
        let parse = quote! {
            let def = #property_def;
            let parsed = ::lumen_engine::json::parse_property(value, &def, #name)
                .map_err(|error| ::anyhow::anyhow!("node {node_id}: {error:#}"))?;
            #reject_expression
            ::lumen_engine::node::DelegateValue::from_property_expression(parsed, node_id, #name)?
        };
        let set = if is_optional_type(ty) {
            let delegate = quote!(<#ty as ::lumen_engine::node::Delegated>::Delegate);
            quote! {
                metas.#ident = if value.is_null()
                    && <#delegate as ::lumen_engine::node::OptionalDelegate>::NULLABLE
                {
                    ::lumen_engine::node::OptionalDelegate::from_override(
                        ::lumen_engine::node::Override::Clear,
                    )
                } else {
                    ::lumen_engine::node::OptionalDelegate::from_override(
                        ::lumen_engine::node::Override::Set({ #parse }),
                    )
                };
            }
        } else {
            quote!(metas.#ident = { #parse };)
        };
        quote! {
            if let Some(value) = params.and_then(|params| params.get(#name)) {
                #set
            }
        }
    });
    let nested_json = delegate.properties.then(|| {
        let json_gets = metas.iter().map(|meta| {
            let ident = &meta.ident;
            let name = &meta.property.id;
            let ty = &meta.ty;
            if is_vec_type(ty) {
                return quote! {
                    fields.insert(
                        #name.to_string(),
                        ::serde_json::Value::Array(
                            self.#ident
                                .items
                                .iter()
                                .map(|item| ::serde_json::Value::Object(item.to_json()))
                                .collect(),
                        ),
                    );
                };
            }
            if is_optional_type(ty) {
                quote! {
                    match ::lumen_engine::node::OptionalDelegate::as_override(&self.#ident) {
                        ::lumen_engine::node::Override::Inherit => {}
                        ::lumen_engine::node::Override::Clear => {
                            fields.insert(#name.to_string(), ::serde_json::Value::Null);
                        }
                        ::lumen_engine::node::Override::Set(value) => {
                            fields.insert(
                                #name.to_string(),
                                ::lumen_engine::node::DelegateValue::to_json_value(value),
                            );
                        }
                    }
                }
            } else {
                quote! {
                    fields.insert(
                        #name.to_string(),
                        ::lumen_engine::node::DelegateValue::to_json_value(&self.#ident),
                    );
                }
            }
        });
        let nested_json_sets = metas.iter().map(|meta| {
            let ident = &meta.ident;
            let name = &meta.property.id;
            let ty = &meta.ty;
            let parse = if is_vec_type(ty) {
                quote! {
                    ::serde_json::from_value(value.clone())
                        .map_err(|error| format!("`{}`: {error}", #name))
                }
            } else {
                let property_def = property_def_tokens(&meta.property, ty);
                let constant = meta.property.constant;
                let parse_field = if is_optional_type(ty) {
                    quote!(::lumen_engine::json::parse_nested_optional_field)
                } else {
                    quote!(::lumen_engine::json::parse_nested_field)
                };
                quote!(#parse_field(value, &#property_def, #constant))
            };
            quote! {
                #name => {
                    delegate.#ident = #parse.map_err(<D::Error as ::serde::de::Error>::custom)?;
                }
            }
        });

        let field_ids = metas.iter().map(|meta| &meta.property.id);
        let required_ids = metas
            .iter()
            .filter(|meta| meta.property.required)
            .map(|meta| &meta.property.id);
        quote! {
            impl #delegate_ident {
                /// The params as a JSON object that deserializing parses back. Unset optional
                /// fields are left out.
                pub fn to_json(&self) -> ::serde_json::Map<::std::string::String, ::serde_json::Value> {
                    let mut fields = ::serde_json::Map::new();
                    #(#json_gets)*
                    fields
                }
            }

            // Parses the nested delegate field by field the way node params parse.
            impl<'de> ::serde::Deserialize<'de> for #delegate_ident {
                fn deserialize<D>(deserializer: D) -> ::core::result::Result<Self, D::Error>
                where
                    D: ::serde::Deserializer<'de>,
                {
                    let fields = <::serde_json::Map<::std::string::String, ::serde_json::Value> as ::serde::Deserialize>::deserialize(deserializer)?;
                    for field in [#(#required_ids),*] {
                        if !fields.contains_key(field) {
                            return Err(<D::Error as ::serde::de::Error>::missing_field(field));
                        }
                    }
                    let mut delegate = <Self as ::core::default::Default>::default();
                    for (key, value) in &fields {
                        match key.as_str() {
                            #(#nested_json_sets)*
                            _ => {
                                return Err(<D::Error as ::serde::de::Error>::unknown_field(
                                    key,
                                    &[#(#field_ids),*],
                                ));
                            }
                        }
                    }
                    Ok(delegate)
                }
            }
        }
    });
    let serde_derive =
        (!delegate.properties).then(|| quote!(#[derive(::serde::Deserialize)] #[serde(default)]));
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
        #serde_derive
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

            fn default_properties(&self) -> ::std::vec::Vec<(&'static str, ::serde_json::Value)> {
                let mut properties = ::std::vec::Vec::new();
                #(#default_properties)*
                properties
            }

            fn get_property(&self, id: &str) -> ::core::option::Option<::lumen_engine::node::PropertyRead<'_>> {
                match id {
                    #(#property_matches,)*
                    _ => None,
                }
            }

            fn from_json(
                node_id: ::lumen_engine::node::NodeId,
                params: ::core::option::Option<&::serde_json::Map<::std::string::String, ::serde_json::Value>>,
            ) -> ::anyhow::Result<Self>
            where
                Self: Sized,
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
                    #(#nested_evaluated_fields,)*
                })
            }

            fn has_expressions(&self) -> bool {
                false #(|| ::lumen_engine::node::DelegateEvaluable::has_expressions(&self.#field_idents))*
            }
        }

        #nested_json

        const _: fn() = || {
            fn __lumen_delegate_assert_delegated<T: ::lumen_engine::node::Delegated>() {}
            #(#assert_field_types)*
        };
    })
}
