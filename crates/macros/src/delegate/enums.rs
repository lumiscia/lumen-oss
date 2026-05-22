use super::*;

fn enum_delegate_variant_tokens(variant: &Variant) -> Result<proc_macro2::TokenStream> {
    let ident = &variant.ident;
    Ok(match &variant.fields {
        Fields::Unit => quote!(#ident),
        Fields::Unnamed(fields) => {
            let fields = fields.unnamed.iter().map(|field| {
                let ty = &field.ty;
                quote!(<#ty as ::lumen_engine::node::Delegated>::Delegate)
            });
            quote!(#ident(#(#fields),*))
        }
        Fields::Named(fields) => {
            let fields = fields.named.iter().map(|field| {
                let ident = field.ident.as_ref().expect("named field");
                let ty = &field.ty;
                quote!(#ident: <#ty as ::lumen_engine::node::Delegated>::Delegate)
            });
            quote!(#ident { #(#fields),* })
        }
    })
}

fn enum_from_match_tokens(
    enum_ident: &Ident,
    variant: &Variant,
) -> Result<proc_macro2::TokenStream> {
    let ident = &variant.ident;
    Ok(match &variant.fields {
        Fields::Unit => quote!(#enum_ident::#ident => Self::#ident),
        Fields::Unnamed(fields) => {
            let vars = (0..fields.unnamed.len())
                .map(|index| format_ident!("field_{index}"))
                .collect::<Vec<_>>();
            quote!(
                #enum_ident::#ident(#(#vars),*) => Self::#ident(
                    #(::lumen_engine::node::Delegated::into_delegate(#vars)),*
                )
            )
        }
        Fields::Named(fields) => {
            let vars = fields
                .named
                .iter()
                .map(|field| field.ident.clone().expect("named field"))
                .collect::<Vec<_>>();
            quote!(
                #enum_ident::#ident { #(#vars),* } => Self::#ident {
                    #(#vars: ::lumen_engine::node::Delegated::into_delegate(#vars)),*
                }
            )
        }
    })
}

fn enum_eval_match_tokens(
    enum_ident: &Ident,
    variant: &Variant,
) -> Result<proc_macro2::TokenStream> {
    let ident = &variant.ident;
    Ok(match &variant.fields {
        Fields::Unit => quote!(Self::#ident => Ok(#enum_ident::#ident)),
        Fields::Unnamed(fields) => {
            let vars = (0..fields.unnamed.len())
                .map(|index| format_ident!("field_{index}"))
                .collect::<Vec<_>>();
            quote!(
                Self::#ident(#(#vars),*) => Ok(#enum_ident::#ident(
                    #(::lumen_engine::node::DelegateValue::eval(#vars, ctx)?),*
                ))
            )
        }
        Fields::Named(fields) => {
            let vars = fields
                .named
                .iter()
                .map(|field| field.ident.clone().expect("named field"))
                .collect::<Vec<_>>();
            quote!(
                Self::#ident { #(#vars),* } => Ok(#enum_ident::#ident {
                    #(#vars: ::lumen_engine::node::DelegateValue::eval(#vars, ctx)?),*
                })
            )
        }
    })
}

fn enum_field_types(variants: &Punctuated<Variant, Token![,]>) -> Vec<Type> {
    variants
        .iter()
        .flat_map(|variant| match &variant.fields {
            Fields::Unit => Vec::new(),
            Fields::Unnamed(fields) => fields
                .unnamed
                .iter()
                .map(|field| field.ty.clone())
                .collect::<Vec<_>>(),
            Fields::Named(fields) => fields
                .named
                .iter()
                .map(|field| field.ty.clone())
                .collect::<Vec<_>>(),
        })
        .collect()
}
pub(crate) fn expand_enum_delegate(
    input: &DeriveInput,
    variants: &Punctuated<Variant, Token![,]>,
) -> Result<proc_macro2::TokenStream> {
    let ident = &input.ident;
    let delegate = parse_delegate_attr(input)?;
    let delegate_ident = delegate.name.clone();
    let has_kind = delegate.kind.is_some();
    let is_enum_kind = delegate
        .kind
        .as_ref()
        .is_some_and(|k| k == "enum" || k == "Enum");
    let node_meta_type_impl = if let Some(kind) = &delegate.kind {
        let property_kind = property_kind_tokens(kind);
        let enum_def_method: Option<proc_macro2::TokenStream> = if is_enum_kind {
            Some(quote! {
                #[cfg(any(feature = "json", feature = "metadata"))]
                fn enum_def() -> ::core::option::Option<&'static ::lumen_engine::node::EnumDef> {
                    ::core::option::Option::Some(<#ident as ::lumen_engine::node::NodeEnum>::enum_def())
                }
            })
        } else {
            None
        };
        quote! {
            impl ::lumen_engine::node::NodeParamType for #ident {
                fn property_kind() -> ::lumen_engine::node::PropertyKind {
                    #property_kind
                }
                #enum_def_method
            }
        }
    } else {
        quote! {
            impl ::lumen_engine::node::NodeParamType for #ident {
                fn property_kind() -> ::lumen_engine::node::PropertyKind {
                    ::lumen_engine::node::PropertyKind::String
                }
            }
        }
    };
    let delegate_variants = variants
        .iter()
        .map(enum_delegate_variant_tokens)
        .collect::<Result<Vec<_>>>()?;
    let from_matches = variants
        .iter()
        .map(|variant| enum_from_match_tokens(ident, variant))
        .collect::<Result<Vec<_>>>()?;
    let eval_matches = variants
        .iter()
        .map(|variant| enum_eval_match_tokens(ident, variant))
        .collect::<Result<Vec<_>>>()?;
    let assert_field_types = enum_field_types(variants)
        .into_iter()
        .map(|ty| quote!(__lumen_delegate_assert_delegated::<#ty>();))
        .collect::<Vec<_>>();

    let enum_property_methods = if has_kind {
        quote! {
            fn to_property_value(&self) -> ::lumen_engine::node::PropertyValue {
                <#ident as ::lumen_engine::node::DeferredValue>::to_property_value(
                    &self
                        .clone()
                        .into_evaluated()
                        .expect("delegate enum property values cannot contain expressions"),
                )
            }

            fn from_property_expression(
                value: ::lumen_engine::node::PropertyExpression,
            ) -> ::core::result::Result<Self, ::lumen_engine::error::LumenError> {
                match value {
                    ::lumen_engine::node::PropertyExpression::Value(value) => {
                        <#ident as ::lumen_engine::node::DeferredValue>::from_property_value(value)
                            .map(::core::convert::Into::into)
                            .ok_or_else(|| ::lumen_engine::error::LumenError::Property(
                                ::lumen_engine::error::PropertyError::InvalidType {
                                    node_id: ::lumen_engine::node::NodeId::new(0),
                                    property_path: ::std::string::String::new(),
                                    expected: <#ident as ::lumen_engine::node::DeferredValue>::property_kind_name(),
                                    actual: "property",
                                }
                            ))
                    }
                    ::lumen_engine::node::PropertyExpression::Expr(_) => {
                        Err(::lumen_engine::error::LumenError::Property(
                            ::lumen_engine::error::PropertyError::InvalidType {
                                node_id: ::lumen_engine::node::NodeId::new(0),
                                property_path: ::std::string::String::new(),
                                expected: <#ident as ::lumen_engine::node::DeferredValue>::property_kind_name(),
                                actual: "expression",
                            }
                        ))
                    }
                }
            }
        }
    } else {
        quote! {
            fn to_property_value(&self) -> ::lumen_engine::node::PropertyValue {
                ::lumen_engine::node::PropertyValue::String(::std::string::String::new())
            }

            fn from_property_expression(
                _value: ::lumen_engine::node::PropertyExpression,
            ) -> ::core::result::Result<Self, ::lumen_engine::error::LumenError> {
                Ok(<Self as ::core::default::Default>::default())
            }
        }
    };

    Ok(quote! {
        #[derive(Debug, Clone)]
        #[cfg_attr(feature = "json", derive(::serde::Deserialize))]
        pub enum #delegate_ident {
            #(#delegate_variants,)*
        }

        impl ::core::convert::From<#ident> for #delegate_ident {
            fn from(value: #ident) -> Self {
                match value {
                    #(#from_matches,)*
                }
            }
        }

        impl ::core::default::Default for #delegate_ident {
            fn default() -> Self {
                <#ident as ::core::default::Default>::default().into()
            }
        }

        impl #delegate_ident {
            pub fn try_into_evaluated(
                &self,
                ctx: &::lumen_engine::node::DelegateEvalContext<'_>,
            ) -> ::core::result::Result<#ident, ::lumen_engine::error::LumenError> {
                match self {
                    #(#eval_matches,)*
                }
            }

            pub fn into_evaluated(
                self,
            ) -> ::core::result::Result<#ident, ::lumen_engine::error::LumenError> {
                let expr = ::lumen_engine::expr::ExpressionContext::default();
                let ctx = ::lumen_engine::node::DelegateEvalContext {
                    node_id: ::lumen_engine::node::NodeId::new(0),
                    property_path: "",
                    expr: &expr,
                };
                self.try_into_evaluated(&ctx)
            }
        }

        impl ::lumen_engine::node::Delegated for #ident {
            type Delegate = #delegate_ident;

            fn into_delegate(self) -> Self::Delegate {
                self.into()
            }
        }

        impl ::lumen_engine::node::DelegateValue for #delegate_ident {
            type Evaluated = #ident;

            fn eval(
                &self,
                ctx: &::lumen_engine::node::DelegateEvalContext<'_>,
            ) -> ::core::result::Result<Self::Evaluated, ::lumen_engine::error::LumenError> {
                self.try_into_evaluated(ctx)
            }

            #enum_property_methods
        }

        #node_meta_type_impl

        const _: fn() = || {
            fn __lumen_delegate_assert_delegated<T: ::lumen_engine::node::Delegated>() {}
            #(#assert_field_types)*
        };
    })
}
