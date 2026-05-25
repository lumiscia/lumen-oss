use super::*;

pub(crate) fn expand_node(input: DeriveInput) -> Result<proc_macro2::TokenStream> {
    let ident = input.ident;
    let node = parse_node_attr(&input.attrs)?;
    let fields = named_fields(&input.data)?;
    let mut inputs = Vec::new();
    let mut properties = Vec::new();
    let mut metas = Vec::new();

    for field in fields {
        let Some(field_ident) = &field.ident else {
            continue;
        };
        if let Some(input) = parse_input_attr(&field.attrs, field_ident)? {
            inputs.push(input);
        }
        if let Some(property) = parse_property_attr(&field.attrs, field_ident)? {
            properties.push(property);
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
        let kind = port_kind_tokens(&input.kind);
        let optional = input.optional;
        let variadic = input.variadic;
        quote! {
            ::lumen_engine::node::InputPortDef {
                name: #name,
                kind: #kind,
                optional: #optional,
                variadic: #variadic,
            }
        }
    });
    let property_defs = properties
        .iter()
        .map(property_def_tokens)
        .collect::<Vec<_>>();
    let metas_property_defs = metas
        .iter()
        .map(|(_, ty)| quote!(<#ty as ::lumen_engine::node::NodeParams>::property_defs()));
    let default_properties = properties.iter().map(|property| {
        let name = &property.id;
        let field = &property.field;
        quote!((#name, defaults.#field.clone()))
    });
    let metas_default_properties = metas
        .iter()
        .map(|(field, _)| quote!(defaults.#field.default_properties()));
    let property_matches = properties.iter().map(|property| {
        let name = &property.id;
        let field = &property.field;
        quote!(#name => Some(self.#field.clone()))
    });
    let metas_property_matches = metas.iter().map(|(field, _)| {
        quote! {
            if let Some(value) = self.#field.get_property(id) {
                return Ok(Some(value));
            }
        }
    });
    let json_property_sets = properties.iter().map(|property| {
        let name = &property.id;
        let field = &property.field;
        let property_def = property_def_tokens(property);
        quote! {
            if let Some(value) = params.and_then(|params| params.get(#name)) {
                let def = #property_def;
                node.#field = ::lumen_engine::json::parse_property(value, Some(&def), #name)?;
            }
        }
    });
    let json_metas_sets = metas.iter().map(|(field, _)| {
        quote! {
            node.#field = ::lumen_engine::node::NodeParams::from_json(params)?;
        }
    });
    let json_metas_known = metas
        .iter()
        .map(|(_, ty)| quote!(<#ty as ::lumen_engine::node::NodeParams>::is_property(key)));
    let json_known_properties = properties.iter().map(|property| {
        let name = &property.id;
        quote!(#name => ())
    });
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
                self.#field = source;
                Ok(())
            })
        }
    });
    let input_port_collectors = inputs.iter().map(|input| {
        let field = &input.field;
        if input.variadic {
            quote!(ports.extend(self.#field.clone()))
        } else {
            quote!(ports.push(self.#field.clone()))
        }
    });

    let kind = node.kind;
    let node_name = node.name;
    let description = doc_string(&node.docs);
    let category = category_tokens(&node.category);

    Ok(quote! {
        const #input_static: &[::lumen_engine::node::InputPortDef] = &[#(#input_defs),*];

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
                        let mut properties = vec![#(#property_defs),*];
                        #(properties.extend(#metas_property_defs);)*
                        properties
                    },
                    default_properties: {
                        let mut default_properties = vec![#(#default_properties),*];
                        #(default_properties.extend(#metas_default_properties);)*
                        default_properties
                    },
                }
            }
        }

        #[cfg(feature = "json")]
        impl ::lumen_engine::node::JsonNode for #ident {
            fn from_json(
                id: ::lumen_engine::node::NodeId,
                params: Option<&::serde_json::Map<String, ::serde_json::Value>>,
            ) -> ::anyhow::Result<Self> {
                let mut node = <Self as ::core::default::Default>::default();
                node.id = id;

                if let Some(params) = params {
                    for key in params.keys() {
                        match key.as_str() {
                            #(#json_known_properties,)*
                            _ if false #(|| #json_metas_known)* => (),
                            _ => ::anyhow::bail!("unknown param `{key}` on node {id}"),
                        };
                    }
                }

                #(#json_property_sets)*
                #(#json_metas_sets)*
                Ok(node)
            }

            fn set_input_json(
                &mut self,
                port: &str,
                source: ::lumen_engine::node::PortRef,
            ) -> ::anyhow::Result<()> {
                match port {
                    #(#json_input_matches,)*
                    _ => ::anyhow::bail!("unknown input port `{port}` on node {}", self.id),
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

            fn input_ports(&self) -> ::std::vec::Vec<::lumen_engine::node::PortRef> {
                let mut ports = ::std::vec::Vec::new();
                #(#input_port_collectors;)*
                ports
            }
        }

        impl ::lumen_engine::node::PropertyEval for #ident {
            fn get_property(
                &self,
                id: &str,
            ) -> ::core::result::Result<
                ::core::option::Option<::lumen_engine::node::PropertyExpression>,
                ::lumen_engine::error::LumenError,
            > {
                #(#metas_property_matches)*
                Ok(match id {
                    #(#property_matches,)*
                    _ => None,
                })
            }
        }
    })
}
