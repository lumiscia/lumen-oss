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

    if let Some(second) = inputs.iter().filter(|input| input.variadic).nth(1) {
        return Err(syn::Error::new(
            second.field.span(),
            "Node derive supports at most one variadic input",
        ));
    }
    if metas.len() > 1 {
        return Err(syn::Error::new(
            metas[1].0.span(),
            "Node derive supports at most one #[params] field",
        ));
    }

    if node.outputs.len() > 31 {
        return Err(syn::Error::new(
            node.outputs[31].0.span(),
            "Node derive supports at most 31 named outputs",
        ));
    }
    let output_static = format_ident!("__LUMEN_{}_OUTPUTS", ident.to_string().to_uppercase());
    let output_defs = node.outputs.iter().map(|(name, kind)| {
        let name = name.to_string();
        let kind = port_kind_tokens(kind.as_ref().or(node.output.as_ref()));
        quote! {
            ::lumen_engine::node::OutputPortDef {
                name: #name,
                kind: #kind,
            }
        }
    });
    let input_static = format_ident!("__LUMEN_{}_INPUTS", ident.to_string().to_uppercase());
    let input_defs = inputs.iter().map(|input| {
        let name = &input.name;
        let kind = port_kind_tokens(input.kind.as_ref());
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
    let metas_property_evaluations = metas.iter().map(|(field, _)| {
        quote! {
            if let Some(value) = ::lumen_engine::node::NodeParams::evaluate_property(
                &self.#field,
                id,
                &::lumen_engine::node::DelegateEvalContext::new(self.id, ctx),
            ) {
                return Some(value);
            }
        }
    });
    let json_metas_sets = metas.iter().map(|(field, _)| {
        quote! {
            match ::lumen_engine::node::NodeParams::from_json(id, params) {
                Ok(value) => node.#field = value,
                Err(param_errors) => errors.extend(param_errors),
            }
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
                        at: ::lumen_engine::error::At::node(self.id),
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

    let metas_fields = metas.iter().map(|(field, _)| field).collect::<Vec<_>>();
    let kind = node.kind;
    let node_name = node.name;
    let description = doc_string(&node.docs);
    let category = category_tokens(&node.category);
    let output = port_kind_tokens(node.output.as_ref());

    Ok(quote! {
        #structural_hash
        const #input_static: &[::lumen_engine::node::InputPortDef] = &[#(#input_defs),*];
        const #output_static: &[::lumen_engine::node::OutputPortDef] = &[#(#output_defs),*];
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
                    output: #output,
                    outputs: #output_static,
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
            ) -> ::core::result::Result<Self, ::std::vec::Vec<::lumen_engine::error::DocumentError>> {
                let mut node = <Self as ::core::default::Default>::default();
                node.id = id;
                let mut errors = ::std::vec::Vec::new();

                if let Some(params) = params {
                    for key in params.keys() {
                        match key.as_str() {
                            _ if false #(|| #json_metas_known)* => (),
                            _ => errors.push(::lumen_engine::error::DocumentError::UnknownParam {
                                at: ::lumen_engine::error::At::param(id, key.as_str()),
                            }),
                        };
                    }
                }

                #(#json_metas_sets)*
                if errors.is_empty() {
                    Ok(node)
                } else {
                    Err(errors)
                }
            }

            fn set_input_json(
                &mut self,
                port: &str,
                source: ::lumen_engine::node::OutputRef,
            ) -> ::core::result::Result<(), ::lumen_engine::error::GraphValidationError> {
                match port {
                    #(#json_input_matches,)*
                    _ => Err(::lumen_engine::error::GraphValidationError::MissingTargetPort {
                        at: ::lumen_engine::error::At::node(self.id),
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

            fn output_kind(&self) -> ::lumen_engine::node::PortKind {
                #output
            }

            fn output_port_defs(&self) -> &'static [::lumen_engine::node::OutputPortDef] {
                #output_static
            }

            fn input_ports(&self) -> ::std::vec::Vec<::core::option::Option<::lumen_engine::node::OutputRef>> {
                let mut ports = ::std::vec::Vec::new();
                #(#input_port_collectors;)*
                ports
            }

            fn for_each_expression(&self, f: &mut dyn FnMut(&::lumen_engine::expr::Expression)) {
                #(::lumen_engine::node::DelegateEvaluable::for_each_expression(&self.#metas_fields, f);)*
            }

            fn for_each_media_source<'a>(&'a self, f: &mut dyn FnMut(::lumen_engine::node::MediaSource<'a>)) {
                #(::lumen_engine::node::NodeParams::for_each_media_source(&self.#metas_fields, f);)*
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

            fn evaluate_property(
                &self,
                id: &str,
                ctx: &::lumen_engine::expr::ExpressionContext<'_>,
            ) -> ::core::option::Option<::core::result::Result<::serde_json::Value, ::lumen_engine::error::LumenError>> {
                #(#metas_property_evaluations)*
                None
            }
        }
    })
}
