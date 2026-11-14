use quote::{format_ident, quote};
use syn::{DeriveInput, Expr, Fields, LitStr, Result};

/// A test that compares the struct's size and field offsets with the WGSL struct it mirrors, named
/// after the struct. Fields whose names start with `_` are padding the shader needn't name.
pub(crate) fn expand(input: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    let ident = &input.ident;
    let syn::Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            ident,
            "ShaderLayout only supports structs",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            ident,
            "ShaderLayout requires named struct fields",
        ));
    };
    if !is_repr_c(input)? {
        return Err(syn::Error::new_spanned(
            ident,
            "ShaderLayout requires `#[repr(C)]`",
        ));
    }
    let (wgsl, source) = parse_attribute(input)?;

    let members = fields
        .named
        .iter()
        .filter_map(|field| field.ident.as_ref())
        .filter(|field| !field.to_string().starts_with('_'))
        .map(|field| {
            let name = LitStr::new(&field.to_string(), field.span());
            quote!((#name, ::core::mem::offset_of!(#ident, #field)))
        });
    let test = format_ident!("{}_matches_shader", snake_case(&ident.to_string()));
    Ok(quote! {
        #[cfg(test)]
        #[test]
        fn #test() {
            crate::gpu::shader_layouts::assert_layout(
                &(#source),
                #wgsl,
                ::core::mem::size_of::<#ident>(),
                &[#(#members),*],
            );
        }
    })
}

fn is_repr_c(input: &DeriveInput) -> Result<bool> {
    let mut repr_c = false;
    for attr in &input.attrs {
        if attr.path().is_ident("repr") {
            attr.parse_nested_meta(|meta| {
                repr_c |= meta.path.is_ident("C");
                // Arguments such as `align(16)` are read past, not understood.
                if meta.input.peek(syn::token::Paren) {
                    let _content;
                    syn::parenthesized!(_content in meta.input);
                }
                Ok(())
            })?;
        }
    }
    Ok(repr_c)
}

fn parse_attribute(input: &DeriveInput) -> Result<(LitStr, Expr)> {
    let mut wgsl = None;
    let mut source = None;
    for attr in &input.attrs {
        if !attr.path().is_ident("shader_layout") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("wgsl") {
                wgsl = Some(meta.value()?.parse::<LitStr>()?);
            } else if meta.path.is_ident("source") {
                source = Some(meta.value()?.parse::<Expr>()?);
            } else {
                return Err(meta.error("expected `wgsl` or `source`"));
            }
            Ok(())
        })?;
    }
    match (wgsl, source) {
        (Some(wgsl), Some(source)) => Ok((wgsl, source)),
        _ => Err(syn::Error::new_spanned(
            &input.ident,
            "ShaderLayout requires `#[shader_layout(wgsl = \"Name\", source = EXPR)]`",
        )),
    }
}

fn snake_case(name: &str) -> String {
    let mut snake = String::new();
    for (index, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if index > 0 {
                snake.push('_');
            }
            snake.push(c.to_ascii_lowercase());
        } else {
            snake.push(c);
        }
    }
    snake
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    fn tokens(input: DeriveInput) -> String {
        expand(&input).expect("the derive expands").to_string()
    }

    fn error(input: DeriveInput) -> String {
        expand(&input)
            .expect_err("the derive is rejected")
            .to_string()
    }

    #[test]
    fn the_test_names_the_struct_and_checks_every_field_but_padding() {
        let tokens = tokens(parse_quote! {
            #[repr(C)]
            #[shader_layout(wgsl = "Params", source = SHADER)]
            struct ExposureUniforms {
                exposure: f32,
                _pad: u32,
                offset: f32,
            }
        });
        assert!(
            tokens.contains("fn exposure_uniforms_matches_shader"),
            "{tokens}"
        );
        assert!(tokens.contains("& (SHADER)"), "{tokens}");
        assert!(tokens.contains("\"Params\""), "{tokens}");
        assert!(tokens.contains("\"exposure\""), "{tokens}");
        assert!(tokens.contains("\"offset\""), "{tokens}");
        assert!(!tokens.contains("_pad"), "{tokens}");
    }

    #[test]
    fn it_rejects_what_it_cannot_check() {
        assert!(
            error(parse_quote! {
                #[shader_layout(wgsl = "Params", source = SHADER)]
                struct Uniforms { a: f32 }
            })
            .contains("repr(C)")
        );
        assert!(
            error(parse_quote! {
                #[repr(C)]
                struct Uniforms { a: f32 }
            })
            .contains("shader_layout(wgsl")
        );
        assert!(
            error(parse_quote! {
                #[repr(C)]
                #[shader_layout(wgsl = "Params", source = SHADER)]
                struct Uniforms(f32);
            })
            .contains("named struct fields")
        );
    }

    #[test]
    fn names_are_snake_case() {
        assert_eq!(snake_case("GpuRepeatCopy"), "gpu_repeat_copy");
        assert_eq!(snake_case("PathPoint"), "path_point");
    }
}
