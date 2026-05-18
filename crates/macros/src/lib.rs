//! Node schema proc macros.
//!
//! `#[derive(Node)]` turns a renderer-neutral node struct into canonical
//! schema metadata plus property lookup glue. It intentionally does not model
//! UI concerns or multiple outputs.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::{
    Attribute, Data, DeriveInput, Expr, ExprLit, Fields, Ident, ImplItem, ItemImpl, LitBool,
    LitFloat, LitInt, LitStr, Path, Result, Token, Type, parse_macro_input, punctuated::Punctuated,
    spanned::Spanned,
};

#[proc_macro_derive(Node, attributes(node, input, property, params))]
pub fn derive_node(input: TokenStream) -> TokenStream {
    match expand_node(parse_macro_input!(input as DeriveInput)) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[proc_macro_derive(NodeParams, attributes(params, param))]
pub fn derive_node_params(input: TokenStream) -> TokenStream {
    match expand_node_params(parse_macro_input!(input as DeriveInput)) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[proc_macro_derive(NodeEnum, attributes(enum_option))]
pub fn derive_node_enum(input: TokenStream) -> TokenStream {
    match expand_node_enum(parse_macro_input!(input as DeriveInput)) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn node_impl(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(item as ItemImpl);
    for item in &mut input.items {
        if let ImplItem::Fn(method) = item {
            method.attrs.retain(|attr| !attr.path().is_ident("output"));
        }
    }
    quote!(#input).into()
}

fn expand_node(input: DeriveInput) -> Result<proc_macro2::TokenStream> {
    let ident = input.ident;
    let node = parse_node_attr(&input.attrs)?;
    let fields = named_fields(&input.data)?;
    let mut inputs = Vec::new();
    let mut properties = Vec::new();
    let mut params = Vec::new();

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
            params.push((field_ident.clone(), field.ty.clone()));
        }
    }

    if params.len() > 1 {
        return Err(syn::Error::new(
            params[1].0.span(),
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
    let params_property_defs = params
        .iter()
        .map(|(_, ty)| quote!(<#ty as ::lumen_engine::node::NodeParams>::property_defs()));
    let default_properties = properties.iter().map(|property| {
        let name = &property.id;
        let field = &property.field;
        quote!((#name, defaults.#field.clone()))
    });
    let params_default_properties = params
        .iter()
        .map(|(field, _)| quote!(defaults.#field.default_properties()));
    let property_matches = properties.iter().map(|property| {
        let name = &property.id;
        let field = &property.field;
        quote!(#name => Some(self.#field.clone()))
    });
    let params_property_matches = params.iter().map(|(field, _)| {
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
            if let Some(value) = properties.and_then(|properties| properties.get(#name)) {
                let def = #property_def;
                node.#field = ::lumen_engine::json::parse_property(value, Some(&def), #name)?;
            }
        }
    });
    let json_params_sets = params.iter().map(|(field, _)| {
        quote! {
            node.#field = ::lumen_engine::node::NodeParams::from_json(properties)?;
        }
    });
    let json_params_known = params
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
                        #(properties.extend(#params_property_defs);)*
                        properties
                    },
                    default_properties: {
                        let mut default_properties = vec![#(#default_properties),*];
                        #(default_properties.extend(#params_default_properties);)*
                        default_properties
                    },
                }
            }
        }

        #[cfg(feature = "json")]
        impl ::lumen_engine::node::JsonNode for #ident {
            fn from_json(
                id: ::lumen_engine::node::NodeId,
                properties: Option<&::serde_json::Map<String, ::serde_json::Value>>,
            ) -> ::anyhow::Result<Self> {
                let mut node = <Self as ::core::default::Default>::default();
                node.id = id;

                if let Some(properties) = properties {
                    for key in properties.keys() {
                        match key.as_str() {
                            #(#json_known_properties,)*
                            _ if false #(|| #json_params_known)* => (),
                            _ => ::anyhow::bail!("unknown property `{key}` on node {id}"),
                        };
                    }
                }

                #(#json_property_sets)*
                #(#json_params_sets)*
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
        }

        impl ::lumen_engine::node::PropertyEval for #ident {
            fn get_property(
                &self,
                id: &str,
            ) -> ::core::result::Result<
                ::core::option::Option<::lumen_engine::node::NodeProperty>,
                ::lumen_engine::error::LumenError,
            > {
                #(#params_property_matches)*
                Ok(match id {
                    #(#property_matches,)*
                    _ => None,
                })
            }
        }
    })
}

fn expand_node_params(input: DeriveInput) -> Result<proc_macro2::TokenStream> {
    let ident = input.ident;
    let evaluated_ident = parse_params_attr(&input.attrs)?;
    let fields = named_fields(&input.data)?;
    let mut params = Vec::new();

    for field in fields {
        let Some(field_ident) = &field.ident else {
            continue;
        };
        let property = parse_param_attr(&field.attrs, field_ident, &field.ty)?;
        params.push(property);
    }

    let evaluated_fields = params.iter().map(|param| {
        let field = &param.property.field;
        let ty = &param.ty;
        quote!(pub #field: #ty)
    });
    let property_defs = params
        .iter()
        .map(|param| property_def_tokens(&param.property));
    let is_property_matches = params.iter().map(|param| {
        let name = &param.property.id;
        quote!(#name)
    });
    let default_properties = params.iter().map(|param| {
        let name = &param.property.id;
        let field = &param.property.field;
        quote! {
            (#name, <#ident as ::core::default::Default>::default().#field.to_node_property())
        }
    });
    let property_matches = params.iter().map(|param| {
        let name = &param.property.id;
        let field = &param.property.field;
        quote!(#name => Some(self.#field.to_node_property()))
    });
    let eval_fields = params.iter().map(|param| {
        let name = &param.property.id;
        let field = &param.property.field;
        quote! {
            #field: self.#field.eval(ctx.node_id, #name, ctx.expr)?
        }
    });

    Ok(quote! {
        #[derive(Debug, Clone)]
        pub struct #evaluated_ident {
            #(#evaluated_fields,)*
        }

        impl ::lumen_engine::node::NodeParams for #ident {
            type Evaluated = #evaluated_ident;

            fn property_defs() -> ::std::vec::Vec<::lumen_engine::node::PropertyDef> {
                vec![#(#property_defs),*]
            }

            fn is_property(id: &str) -> bool {
                matches!(id, #(#is_property_matches)|*)
            }

            fn default_properties(&self) -> ::std::vec::Vec<(&'static str, ::lumen_engine::node::NodeProperty)> {
                vec![#(#default_properties),*]
            }

            fn get_property(&self, id: &str) -> ::core::option::Option<::lumen_engine::node::NodeProperty> {
                match id {
                    #(#property_matches,)*
                    _ => None,
                }
            }

            fn eval(
                &self,
                ctx: &::lumen_engine::node::NodeParamEvalContext<'_>,
            ) -> ::lumen_engine::Result<Self::Evaluated> {
                Ok(#evaluated_ident {
                    #(#eval_fields,)*
                })
            }
        }
    })
}

fn expand_node_enum(input: DeriveInput) -> Result<proc_macro2::TokenStream> {
    let ident = input.ident;
    let variants = match input.data {
        Data::Enum(data) => data.variants,
        _ => {
            return Err(syn::Error::new(
                Span::call_site(),
                "NodeEnum derive only supports enums",
            ));
        }
    };

    let mut next_value = 0_i64;
    let mut options = Vec::new();
    for variant in variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(syn::Error::new(
                variant.fields.span(),
                "NodeEnum derive only supports unit variants",
            ));
        }

        let value = match variant.discriminant {
            Some((_, expr)) => {
                let value = int_expr_value(&expr)?;
                next_value = value + 1;
                value
            }
            None => {
                let value = next_value;
                next_value += 1;
                value
            }
        };
        let option = parse_enum_option_attr(&variant.attrs, &variant.ident, value)?;
        options.push(option);
    }

    let enum_name = ident.to_string();
    let option_tokens = options.iter().map(|option| {
        let name = &option.name;
        let label = &option.label;
        let value = option.value;
        quote! {
            ::lumen_engine::node::EnumOptionDef {
                name: #name,
                label: #label,
                value: #value,
            }
        }
    });

    Ok(quote! {
        #[cfg(any(feature = "json", feature = "metadata"))]
        impl ::lumen_engine::node::NodeEnum for #ident {
            fn enum_def() -> &'static ::lumen_engine::node::EnumDef {
                const OPTIONS: &[::lumen_engine::node::EnumOptionDef] = &[#(#option_tokens),*];
                const DEF: ::lumen_engine::node::EnumDef = ::lumen_engine::node::EnumDef {
                    name: #enum_name,
                    options: OPTIONS,
                };
                &DEF
            }
        }
    })
}

fn named_fields(data: &Data) -> Result<&Punctuated<syn::Field, Token![,]>> {
    match data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => Ok(&fields.named),
            fields => Err(syn::Error::new(
                fields.span(),
                "Node derive requires named struct fields",
            )),
        },
        _ => Err(syn::Error::new(
            Span::call_site(),
            "Node derive only supports structs",
        )),
    }
}

struct NodeAttr {
    kind: LitStr,
    name: LitStr,
    category: Ident,
    docs: Vec<String>,
}

struct InputAttr {
    field: Ident,
    name: LitStr,
    kind: Ident,
    optional: bool,
    variadic: bool,
}

struct PropertyAttr {
    field: Ident,
    id: LitStr,
    name: LitStr,
    kind: Ident,
    enum_type: Option<Path>,
    constraints: PropertyConstraintsAttr,
    docs: Vec<String>,
}

struct ParamAttr {
    property: PropertyAttr,
    ty: Type,
}

#[derive(Default)]
struct PropertyConstraintsAttr {
    min: Option<f64>,
    max: Option<f64>,
    step: Option<f64>,
    format: Option<LitStr>,
    multiline: bool,
    recommended_rows: Option<u32>,
    role: Option<LitStr>,
}

struct EnumOptionAttr {
    name: String,
    label: String,
    value: i64,
}

fn parse_node_attr(attrs: &[Attribute]) -> Result<NodeAttr> {
    let attr = required_attr(attrs, "node")?;
    let mut kind = None;
    let mut name = None;
    let mut category = None;

    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("kind") {
            kind = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("name") {
            name = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("category") {
            category = Some(ident_value(&meta)?);
        } else {
            return Err(meta.error("unknown node attribute key"));
        }
        Ok(())
    })?;

    Ok(NodeAttr {
        kind: required_value(kind, attr, "kind")?,
        name: required_value(name, attr, "name")?,
        category: required_value(category, attr, "category")?,
        docs: doc_lines(attrs),
    })
}

fn parse_input_attr(attrs: &[Attribute], field: &Ident) -> Result<Option<InputAttr>> {
    let Some(attr) = optional_attr(attrs, "input") else {
        return Ok(None);
    };
    let mut name = None;
    let mut kind = None;
    let mut optional = None;
    let mut variadic = None;

    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("name") {
            name = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("kind") {
            kind = Some(ident_value(&meta)?);
        } else if meta.path.is_ident("optional") {
            optional = Some(optional_bool(&meta)?);
        } else if meta.path.is_ident("variadic") {
            variadic = Some(optional_bool(&meta)?);
        } else {
            return Err(meta.error("unknown input attribute key"));
        }
        Ok(())
    })?;

    Ok(Some(InputAttr {
        field: field.clone(),
        name: name.unwrap_or_else(|| LitStr::new(&field.to_string(), field.span())),
        kind: kind.unwrap_or_else(|| Ident::new("raster", Span::call_site())),
        optional: optional.unwrap_or(false),
        variadic: variadic.unwrap_or(false),
    }))
}

fn parse_property_attr(attrs: &[Attribute], field: &Ident) -> Result<Option<PropertyAttr>> {
    parse_property_like_attr(attrs, field, "property")
}

fn parse_params_attr(attrs: &[Attribute]) -> Result<Path> {
    let attr = required_attr(attrs, "params")?;
    let mut evaluated = None;
    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("evaluated") {
            evaluated = Some(meta.value()?.parse()?);
        } else {
            return Err(meta.error("unknown params attribute key"));
        }
        Ok(())
    })?;
    required_value(evaluated, attr, "evaluated")
}

fn parse_param_attr(attrs: &[Attribute], field: &Ident, ty: &Type) -> Result<ParamAttr> {
    let Some(property) = parse_property_like_attr(attrs, field, "param")? else {
        return Err(syn::Error::new(
            field.span(),
            "NodeParams fields require #[param(kind = ...)]",
        ));
    };
    Ok(ParamAttr {
        property,
        ty: deferred_inner_type(ty)?,
    })
}

fn parse_property_like_attr(
    attrs: &[Attribute],
    field: &Ident,
    attr_name: &str,
) -> Result<Option<PropertyAttr>> {
    let Some(attr) = optional_attr(attrs, attr_name) else {
        return Ok(None);
    };
    parse_property_attr_body(attr, attrs, field).map(Some)
}

fn parse_property_attr_body(
    attr: &Attribute,
    attrs: &[Attribute],
    field: &Ident,
) -> Result<PropertyAttr> {
    let mut id = None;
    let mut name = None;
    let mut kind = None;
    let mut enum_type = None;
    let mut constraints = PropertyConstraintsAttr::default();

    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("id") {
            id = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("name") {
            name = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("kind") {
            kind = Some(ident_value(&meta)?);
        } else if meta.path.is_ident("enum_type") {
            enum_type = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("min") {
            constraints.min = Some(float_value(&meta)?);
        } else if meta.path.is_ident("max") {
            constraints.max = Some(float_value(&meta)?);
        } else if meta.path.is_ident("step") {
            constraints.step = Some(float_value(&meta)?);
        } else if meta.path.is_ident("format") {
            constraints.format = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("multiline") {
            constraints.multiline = optional_bool(&meta)?;
        } else if meta.path.is_ident("recommended_rows") {
            constraints.recommended_rows = Some(meta.value()?.parse::<LitInt>()?.base10_parse()?);
        } else if meta.path.is_ident("role") {
            constraints.role = Some(meta.value()?.parse()?);
        } else {
            return Err(meta.error("unknown property attribute key"));
        }
        Ok(())
    })?;

    Ok(PropertyAttr {
        field: field.clone(),
        id: id.unwrap_or_else(|| LitStr::new(&field.to_string(), field.span())),
        name: name
            .unwrap_or_else(|| LitStr::new(&title_from_ident(&field.to_string()), field.span())),
        kind: required_value(kind, attr, "kind")?,
        enum_type,
        constraints,
        docs: doc_lines(attrs),
    })
}

fn deferred_inner_type(ty: &Type) -> Result<Type> {
    let Type::Path(path) = ty else {
        return Err(syn::Error::new(ty.span(), "expected Deferred<T>"));
    };
    let Some(segment) = path.path.segments.last() else {
        return Err(syn::Error::new(ty.span(), "expected Deferred<T>"));
    };
    if segment.ident != "Deferred" {
        return Err(syn::Error::new(ty.span(), "expected Deferred<T>"));
    }
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return Err(syn::Error::new(ty.span(), "expected Deferred<T>"));
    };
    let Some(syn::GenericArgument::Type(inner)) = args.args.first() else {
        return Err(syn::Error::new(ty.span(), "expected Deferred<T>"));
    };
    Ok(inner.clone())
}

fn property_def_tokens(property: &PropertyAttr) -> proc_macro2::TokenStream {
    let id = &property.id;
    let name = &property.name;
    let kind = property_kind_tokens(&property.kind);
    let enum_def = match &property.enum_type {
        Some(enum_type) => quote!(Some(<#enum_type as ::lumen_engine::node::NodeEnum>::enum_def())),
        None => quote!(None),
    };
    let description = doc_string(&property.docs);
    let constraints = property_constraints_tokens(&property.constraints);
    quote! {
        ::lumen_engine::node::PropertyDef {
            id: #id,
            expected: #kind,
            #[cfg(any(feature = "json", feature = "metadata"))]
            enum_def: #enum_def,
            #[cfg(feature = "metadata")]
            name: #name,
            #[cfg(feature = "metadata")]
            description: #description,
            #[cfg(feature = "metadata")]
            constraints: #constraints,
        }
    }
}

fn property_constraints_tokens(constraints: &PropertyConstraintsAttr) -> proc_macro2::TokenStream {
    let min = option_f64_tokens(constraints.min);
    let max = option_f64_tokens(constraints.max);
    let step = option_f64_tokens(constraints.step);
    let format = option_lit_str_tokens(&constraints.format);
    let multiline = constraints.multiline;
    let recommended_rows = match constraints.recommended_rows {
        Some(value) => quote!(Some(#value)),
        None => quote!(None),
    };
    let role = option_lit_str_tokens(&constraints.role);
    quote! {
        ::lumen_engine::node::PropertyConstraints {
            min: #min,
            max: #max,
            step: #step,
            format: #format,
            multiline: #multiline,
            recommended_rows: #recommended_rows,
            role: #role,
        }
    }
}

fn parse_enum_option_attr(
    attrs: &[Attribute],
    variant: &Ident,
    value: i64,
) -> Result<EnumOptionAttr> {
    let mut name = None;
    let mut label = None;

    if let Some(attr) = optional_attr(attrs, "enum_option") {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                name = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("label") {
                label = Some(meta.value()?.parse::<LitStr>()?.value());
            } else {
                return Err(meta.error("unknown enum_option attribute key"));
            }
            Ok(())
        })?;
    }

    let variant_name = variant.to_string();
    Ok(EnumOptionAttr {
        name: name.unwrap_or_else(|| to_snake_case(&variant_name)),
        label: label.unwrap_or_else(|| title_from_ident(&variant_name)),
        value,
    })
}

fn int_expr_value(expr: &Expr) -> Result<i64> {
    match expr {
        Expr::Lit(ExprLit {
            lit: syn::Lit::Int(lit),
            ..
        }) => parse_lit_int(lit),
        _ => Err(syn::Error::new(
            expr.span(),
            "NodeEnum variant discriminants must be integer literals",
        )),
    }
}

fn parse_lit_int(lit: &LitInt) -> Result<i64> {
    lit.base10_parse::<i64>()
}

fn to_snake_case(value: &str) -> String {
    let mut out = String::new();
    for (index, ch) in value.chars().enumerate() {
        if ch.is_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn title_from_ident(value: &str) -> String {
    let mut out = String::new();
    for (index, part) in to_snake_case(value).split('_').enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

fn doc_lines(attrs: &[Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .filter_map(|attr| match &attr.meta {
            syn::Meta::NameValue(name_value) => match &name_value.value {
                syn::Expr::Lit(expr_lit) => match &expr_lit.lit {
                    syn::Lit::Str(lit) => Some(lit.value().trim().to_string()),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn doc_string(lines: &[String]) -> String {
    lines.join("\n")
}

fn required_attr<'a>(attrs: &'a [Attribute], name: &str) -> Result<&'a Attribute> {
    optional_attr(attrs, name)
        .ok_or_else(|| syn::Error::new(Span::call_site(), format!("missing #[{name}(...)]")))
}

fn optional_attr<'a>(attrs: &'a [Attribute], name: &str) -> Option<&'a Attribute> {
    attrs.iter().find(|attr| attr.path().is_ident(name))
}

fn has_attr(attrs: &[Attribute], name: &str) -> bool {
    optional_attr(attrs, name).is_some()
}

fn required_value<T>(value: Option<T>, attr: &Attribute, name: &str) -> Result<T> {
    value.ok_or_else(|| {
        syn::Error::new(
            attr.span(),
            format!(
                "missing `{name}` in #[{}(...)]",
                attr.path().get_ident().unwrap()
            ),
        )
    })
}

fn ident_value(meta: &syn::meta::ParseNestedMeta<'_>) -> Result<Ident> {
    let value = meta.value()?;
    if value.peek(LitStr) {
        let lit: LitStr = value.parse()?;
        Ok(Ident::new(&lit.value(), lit.span()))
    } else {
        value.parse()
    }
}

fn optional_bool(meta: &syn::meta::ParseNestedMeta<'_>) -> Result<bool> {
    if meta.input.peek(Token![=]) {
        Ok(meta.value()?.parse::<LitBool>()?.value)
    } else {
        Ok(true)
    }
}

fn float_value(meta: &syn::meta::ParseNestedMeta<'_>) -> Result<f64> {
    let value = meta.value()?;
    if value.peek(LitFloat) {
        value.parse::<LitFloat>()?.base10_parse()
    } else {
        value
            .parse::<LitInt>()?
            .base10_parse::<i64>()
            .map(|value| value as f64)
    }
}

fn option_f64_tokens(value: Option<f64>) -> proc_macro2::TokenStream {
    match value {
        Some(value) => quote!(Some(#value)),
        None => quote!(None),
    }
}

fn option_lit_str_tokens(value: &Option<LitStr>) -> proc_macro2::TokenStream {
    match value {
        Some(value) => quote!(Some(#value)),
        None => quote!(None),
    }
}

fn category_tokens(category: &Ident) -> proc_macro2::TokenStream {
    match category.to_string().as_str() {
        "compositing" | "Compositing" => quote!(::lumen_engine::node::NodeCategory::Compositing),
        "processing" | "Processing" => quote!(::lumen_engine::node::NodeCategory::Processing),
        "source" | "Source" => quote!(::lumen_engine::node::NodeCategory::Source),
        "output" | "Output" => quote!(::lumen_engine::node::NodeCategory::Output),
        "vector" | "Vector" => quote!(::lumen_engine::node::NodeCategory::Vector),
        _ => {
            let span = category.span();
            quote::quote_spanned!(span=> compile_error!("unknown node category"))
        }
    }
}

fn port_kind_tokens(kind: &Ident) -> proc_macro2::TokenStream {
    match kind.to_string().as_str() {
        "raster" | "Raster" => quote!(::lumen_engine::node::PortKind::Raster),
        "vector" | "Vector" => quote!(::lumen_engine::node::PortKind::Vector),
        _ => {
            let span = kind.span();
            quote::quote_spanned!(span=> compile_error!("unknown port kind"))
        }
    }
}

fn property_kind_tokens(kind: &Ident) -> proc_macro2::TokenStream {
    match kind.to_string().as_str() {
        "float" | "Float" => quote!(::lumen_engine::node::PropertyKind::Float),
        "int" | "Int" => quote!(::lumen_engine::node::PropertyKind::Int),
        "bool" | "Bool" => quote!(::lumen_engine::node::PropertyKind::Bool),
        "string" | "String" => quote!(::lumen_engine::node::PropertyKind::String),
        "color" | "Color" => quote!(::lumen_engine::node::PropertyKind::Color),
        "vec2" | "Vec2" => quote!(::lumen_engine::node::PropertyKind::Vec2),
        "enum" | "Enum" => quote!(::lumen_engine::node::PropertyKind::Enum),
        _ => {
            let span = kind.span();
            quote::quote_spanned!(span=> compile_error!("unknown property kind"))
        }
    }
}
