use super::*;

pub(crate) fn expand_node_enum(input: DeriveInput) -> Result<proc_macro2::TokenStream> {
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
