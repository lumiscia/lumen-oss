use super::*;

pub(crate) fn expand_node(input: DeriveInput) -> Result<proc_macro2::TokenStream> {
    let structural_hash = crate::structural_hash::expand(&input)?;
    let ident = input.ident;
    let node = parse_node_attr(&input.attrs)?;
    let fields = named_fields(&input.data)?;
    let mut inputs = Vec::new();
    let mut metas = Vec::new();

    for field in fields {
        let Some(field_ident) = &field.ident else {
            continue;
        };
        if let Some(input) = parse_input_attr(&field.attrs, field_ident)? {
            inputs.push(input);
        }
        if has_attr(&field.attrs, "params") {
            metas.push((field_ident.clone(), field.ty.clone()));
        }
    }

    if metas.len() > 1 {
        return Err(syn::Error::new(
            metas[1].0.span(),
            "Node derive supports at most one #[params] field",
        ));
    }

    let input_static = format_ident!("__LUMEN_{}_INPUTS", ident.to_string().to_uppercase());
    let input_defs = inputs.iter().map(|input| {
        let name = &input.name;
        let optional = input.optional;
        let variadic = input.variadic;
        quote! {
            ::lumen_engine::node::InputPortDef {
                name: #name,
                optional: #optional,
                variadic: #variadic,
            }
        }
    });
    let metas_property_defs = metas
        .iter()
        .map(|(_, ty)| quote!(<#ty as ::lumen_engine::node::NodeParams>::property_defs()));
    let metas_default_properties = metas
        .iter()
        .map(|(field, _)| quote!(defaults.#field.json_properties()));
    let metas_property_matches = metas.iter().map(|(field, _)| {
        quote! {
            if let Some(value) = self.#field.get_property(id) {
                return Some(value);
            }
        }
    });
    let json_metas_sets = metas.iter().map(|(field, _)| {
        quote! {
            node.#field = ::lumen_engine::node::NodeParams::from_json(id, params)?;
        }
    });
    let assert_no_vec_fields = metas.iter().map(|(_, ty)| {
        quote! {
            const _: () = assert!(
                !<#ty as ::lumen_engine::node::NodeParams>::HAS_VEC_FIELDS,
                "node params cannot have `Vec` fields: the node schema has no array kind",
            );
        }
    });
    let json_metas_known = metas
        .iter()
        .map(|(_, ty)| quote!(<#ty as ::lumen_engine::node::NodeParams>::is_property(key)));
    let json_input_matches = inputs.iter().map(|input| {
        let name = &input.name;
        let field = &input.field;
        if input.variadic {
            quote!(#name => {
                self.#field.push(source);
                Ok(())
            })
        } else {
            quote!(#name => {
                if self.#field.is_some() {
                    return Err(::lumen_engine::error::GraphValidationError::InputAlreadyConnected {
                        node_id: self.id,
                        port: #name.to_string(),
                    });
                }
                self.#field = Some(source);
                Ok(())
            })
        }
    });
    let input_port_collectors = inputs.iter().map(|input| {
        let field = &input.field;
        if input.variadic {
            quote!(ports.extend(self.#field.iter().copied().map(Some)))
        } else {
            quote!(ports.push(self.#field))
        }
    });

    let metas_fields = metas.iter().map(|(field, _)| field);
    let kind = node.kind;
    let node_name = node.name;
    let description = doc_string(&node.docs);
    let category = category_tokens(&node.category);

    Ok(quote! {
        #structural_hash
        const #input_static: &[::lumen_engine::node::InputPortDef] = &[#(#input_defs),*];
        #(#assert_no_vec_fields)*

        #[cfg(feature = "metadata")]
        impl ::lumen_engine::node::NodeSchema for #ident {
            fn schema() -> ::lumen_engine::node::NodeSchemaDef {
                let defaults = <Self as ::core::default::Default>::default();
                ::lumen_engine::node::NodeSchemaDef {
                    kind: #kind,
                    name: #node_name,
                    description: #description,
                    category: #category,
                    inputs: #input_static,
                    properties: {
                        let mut properties = ::std::vec::Vec::new();
                        #(properties.extend(#metas_property_defs);)*
                        properties
                    },
                    default_properties: {
                        let mut default_properties = ::std::vec::Vec::new();
                        #(default_properties.extend(#metas_default_properties);)*
                        default_properties
                    },
                }
            }
        }

        impl ::lumen_engine::node::JsonNode for #ident {
            const KIND: &'static str = #kind;

            fn from_json(
                id: ::lumen_engine::node::NodeId,
                params: Option<&::serde_json::Map<String, ::serde_json::Value>>,
            ) -> ::anyhow::Result<Self> {
                let mut node = <Self as ::core::default::Default>::default();
                node.id = id;

                if let Some(params) = params {
                    for key in params.keys() {
                        match key.as_str() {
                            _ if false #(|| #json_metas_known)* => (),
                            _ => return ::core::result::Result::Err(::lumen_engine::json::unknown_param_error(key, id)),
                        };
                    }
                }

                #(#json_metas_sets)*
                Ok(node)
            }

            fn set_input_json(
                &mut self,
                port: &str,
                source: ::lumen_engine::node::NodeId,
            ) -> ::core::result::Result<(), ::lumen_engine::error::GraphValidationError> {
                match port {
                    #(#json_input_matches,)*
                    _ => Err(::lumen_engine::error::GraphValidationError::MissingTargetPort {
                        node_id: self.id,
                        port: port.to_string(),
                    }),
                }
            }
        }

        impl ::lumen_engine::node::Node for #ident {
            fn id(&self) -> ::lumen_engine::node::NodeId {
                self.id
            }

            fn input_port_defs(&self) -> &'static [::lumen_engine::node::InputPortDef] {
                #input_static
            }

            fn input_ports(&self) -> ::std::vec::Vec<::core::option::Option<::lumen_engine::node::NodeId>> {
                let mut ports = ::std::vec::Vec::new();
                #(#input_port_collectors;)*
                ports
            }

            fn has_expressions(&self) -> bool {
                false #(|| ::lumen_engine::node::DelegateEvaluable::has_expressions(&self.#metas_fields))*
            }
        }

        impl ::lumen_engine::node::PropertyEval for #ident {
            fn get_property(
                &self,
                id: &str,
            ) -> ::core::option::Option<::lumen_engine::node::PropertyRead<'_>> {
                #(#metas_property_matches)*
                None
            }
        }
    })
}
