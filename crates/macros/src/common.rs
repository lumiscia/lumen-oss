//! Attribute parsing and token helpers shared by the schema and delegate macros.

use quote::quote;
use syn::{Attribute, Ident, LitBool, LitFloat, LitInt, LitStr, Result, Token};

#[derive(Default)]
pub(crate) struct PropertyConstraintsAttr {
    min: Option<f64>,
    enforced_min: bool,
    max: Option<f64>,
    step: Option<f64>,
    format: Option<LitStr>,
    multiline: bool,
    recommended_rows: Option<u32>,
    role: Option<LitStr>,
}

impl PropertyConstraintsAttr {
    /// Parses a constraint key, returning `false` when `meta` is not a constraint.
    pub(crate) fn parse_key(&mut self, meta: &syn::meta::ParseNestedMeta<'_>) -> Result<bool> {
        if meta.path.is_ident("min") {
            self.min = Some(float_value(meta)?);
        } else if meta.path.is_ident("enforced_min") {
            self.enforced_min = optional_bool(meta)?;
        } else if meta.path.is_ident("max") {
            self.max = Some(float_value(meta)?);
        } else if meta.path.is_ident("step") {
            self.step = Some(float_value(meta)?);
        } else if meta.path.is_ident("format") {
            self.format = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("multiline") {
            self.multiline = optional_bool(meta)?;
        } else if meta.path.is_ident("recommended_rows") {
            self.recommended_rows = Some(meta.value()?.parse::<LitInt>()?.base10_parse()?);
        } else if meta.path.is_ident("role") {
            self.role = Some(meta.value()?.parse()?);
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    fn tokens(&self) -> proc_macro2::TokenStream {
        let min = option_f64_tokens(self.min);
        let enforced_min = self.enforced_min;
        let max = option_f64_tokens(self.max);
        let step = option_f64_tokens(self.step);
        let format = option_lit_str_tokens(&self.format);
        let multiline = self.multiline;
        let recommended_rows = match self.recommended_rows {
            Some(value) => quote!(Some(#value)),
            None => quote!(None),
        };
        let role = option_lit_str_tokens(&self.role);
        quote! {
            ::lumen_engine::node::PropertyConstraints {
                min: #min,
                enforced_min: #enforced_min,
                max: #max,
                step: #step,
                format: #format,
                multiline: #multiline,
                recommended_rows: #recommended_rows,
                role: #role,
            }
        }
    }
}

/// What `#[media(image, video, by = kind)]` declares: the kinds of media a `String` param
/// reads, and the enum param that picks one of them.
pub(crate) struct MediaAttr {
    kinds: Vec<Ident>,
    pub(crate) by: Option<Ident>,
}

const MEDIA_KINDS: [(&str, &str); 5] = [
    ("image", "Image"),
    ("video", "Video"),
    ("audio", "Audio"),
    ("model", "Model"),
    ("environment", "Environment"),
];

impl MediaAttr {
    /// The `#[media(...)]` among `attrs`, if there is one.
    pub(crate) fn parse(attrs: &[Attribute]) -> Result<Option<Self>> {
        let Some(attr) = optional_attr(attrs, "media") else {
            return Ok(None);
        };
        let mut kinds: Vec<Ident> = Vec::new();
        let mut by = None;
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("by") {
                by = Some(ident_value(&meta)?);
                return Ok(());
            }
            let Some(kind) = meta
                .path
                .get_ident()
                .filter(|ident| MEDIA_KINDS.iter().any(|(name, _)| ident == name))
            else {
                let names = MEDIA_KINDS.map(|(name, _)| name).join(", ");
                return Err(meta.error(format!(
                    "unknown media kind, expected one of {names}, or `by = <param>`"
                )));
            };
            if kinds.contains(kind) {
                return Err(meta.error("media kind listed twice"));
            }
            kinds.push(kind.clone());
            Ok(())
        })?;
        if kinds.is_empty() {
            return Err(syn::Error::new_spanned(
                attr,
                "#[media(...)] needs at least one kind",
            ));
        }
        if by.is_some() && kinds.len() < 2 {
            return Err(syn::Error::new_spanned(
                attr,
                "`by` picks among kinds, so #[media(...)] needs two or more",
            ));
        }
        Ok(Some(Self { kinds, by }))
    }

    /// A `MediaParam` for the param `param`, whose selector's id is `selector`.
    pub(crate) fn tokens(
        &self,
        param: &LitStr,
        selector: Option<&LitStr>,
    ) -> proc_macro2::TokenStream {
        let kinds = self.kinds.iter().map(|kind| {
            let variant = MEDIA_KINDS
                .iter()
                .find(|(name, _)| kind == name)
                .map(|(_, variant)| Ident::new(variant, kind.span()))
                .expect("kinds are checked when parsed");
            quote!(::lumen_engine::media::AssetKind::#variant)
        });
        let selector = match selector {
            Some(selector) => quote!(Some(#selector)),
            None => quote!(None),
        };
        quote! {
            ::lumen_engine::node::MediaParam {
                param: #param,
                kinds: &[#(#kinds),*],
                selector: #selector,
            }
        }
    }
}

pub(crate) struct PropertyDefTokens<'a> {
    pub id: &'a LitStr,
    pub name: &'a LitStr,
    pub kind: proc_macro2::TokenStream,
    pub enum_def: proc_macro2::TokenStream,
    pub media: proc_macro2::TokenStream,
    pub docs: &'a [String],
    pub constraints: &'a PropertyConstraintsAttr,
    pub accepts_expressions: proc_macro2::TokenStream,
    pub nullable: proc_macro2::TokenStream,
    pub required: bool,
}

impl PropertyDefTokens<'_> {
    pub(crate) fn tokens(&self) -> proc_macro2::TokenStream {
        let Self {
            id,
            name,
            kind,
            enum_def,
            media,
            accepts_expressions,
            nullable,
            required,
            ..
        } = self;
        let description = doc_string(self.docs);
        let constraints = self.constraints.tokens();
        quote! {
            ::lumen_engine::node::PropertyDef {
                id: #id,
                expected: #kind,
                enum_def: #enum_def,
                media: #media,
                #[cfg(feature = "metadata")]
                name: #name,
                #[cfg(feature = "metadata")]
                description: #description,
                #[cfg(feature = "metadata")]
                constraints: #constraints,
                #[cfg(feature = "metadata")]
                accepts_expressions: #accepts_expressions,
                #[cfg(feature = "metadata")]
                nullable: #nullable,
                #[cfg(feature = "metadata")]
                required: #required,
            }
        }
    }
}

pub(crate) fn property_kind_tokens(kind: &Ident) -> proc_macro2::TokenStream {
    match kind.to_string().as_str() {
        "float" | "Float" => quote!(::lumen_engine::node::PropertyKind::Float),
        "int" | "Int" => quote!(::lumen_engine::node::PropertyKind::Int),
        "bool" | "Bool" => quote!(::lumen_engine::node::PropertyKind::Bool),
        "string" | "String" => quote!(::lumen_engine::node::PropertyKind::String),
        "color" | "Color" => quote!(::lumen_engine::node::PropertyKind::Color),
        "vec2" | "Vec2" => quote!(::lumen_engine::node::PropertyKind::Vec2),
        "vec3" | "Vec3" => quote!(::lumen_engine::node::PropertyKind::Vec3),
        "enum" | "Enum" => quote!(::lumen_engine::node::PropertyKind::Enum),
        "paint" | "Paint" => quote!(::lumen_engine::node::PropertyKind::Paint),
        "shader_uniforms" | "ShaderUniforms" => {
            quote!(::lumen_engine::node::PropertyKind::ShaderUniforms)
        }
        "clip_props" | "ClipProps" => quote!(::lumen_engine::node::PropertyKind::ClipProps),
        "text_content" | "TextContent" => quote!(::lumen_engine::node::PropertyKind::TextContent),
        "text_line" | "TextLine" => quote!(::lumen_engine::node::PropertyKind::TextLine),
        "text_highlight" | "TextHighlight" => {
            quote!(::lumen_engine::node::PropertyKind::TextHighlight)
        }
        "font_features" | "FontFeatures" => {
            quote!(::lumen_engine::node::PropertyKind::FontFeatures)
        }
        "text_stroke" | "TextStroke" => quote!(::lumen_engine::node::PropertyKind::TextStroke),
        "dash_pattern" | "DashPattern" => quote!(::lumen_engine::node::PropertyKind::DashPattern),
        "repeat" | "Repeat" => quote!(::lumen_engine::node::PropertyKind::Repeat),
        _ => {
            let span = kind.span();
            quote::quote_spanned!(span=> compile_error!("unknown param kind"))
        }
    }
}

pub(crate) fn doc_lines(attrs: &[Attribute]) -> Vec<String> {
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

pub(crate) fn doc_string(lines: &[String]) -> String {
    lines.join("\n")
}

pub(crate) fn to_snake_case(value: &str) -> String {
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

pub(crate) fn title_from_ident(value: &str) -> String {
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

pub(crate) fn optional_attr<'a>(attrs: &'a [Attribute], name: &str) -> Option<&'a Attribute> {
    attrs.iter().find(|attr| attr.path().is_ident(name))
}

pub(crate) fn ident_value(meta: &syn::meta::ParseNestedMeta<'_>) -> Result<Ident> {
    let value = meta.value()?;
    if value.peek(LitStr) {
        let lit: LitStr = value.parse()?;
        Ok(Ident::new(&lit.value(), lit.span()))
    } else {
        value.parse()
    }
}

pub(crate) fn optional_bool(meta: &syn::meta::ParseNestedMeta<'_>) -> Result<bool> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn media(attr: proc_macro2::TokenStream) -> Result<Option<MediaAttr>> {
        let field: syn::Field =
            syn::parse::Parser::parse2(syn::Field::parse_named, quote!(#attr pub source: String))?;
        MediaAttr::parse(&field.attrs)
    }

    fn error(attr: proc_macro2::TokenStream) -> String {
        media(attr)
            .err()
            .expect("the attribute is rejected")
            .to_string()
    }

    #[test]
    fn media_lists_kinds_and_a_selector() {
        let attr = media(quote!(#[media(image, video, by = kind)]))
            .unwrap()
            .unwrap();
        assert_eq!(attr.kinds, ["image", "video"]);
        assert_eq!(attr.by.unwrap(), "kind");
        assert!(media(quote!(#[meta()])).unwrap().is_none());
    }

    #[test]
    fn media_tokens_name_the_kinds_the_param_and_its_selector() {
        let attr = media(quote!(#[media(model, environment)]))
            .unwrap()
            .unwrap();
        let param = LitStr::new("source", proc_macro2::Span::call_site());
        let selector = LitStr::new("kind", proc_macro2::Span::call_site());
        let tokens = attr.tokens(&param, Some(&selector)).to_string();
        assert!(
            tokens.contains(
                "AssetKind :: Model , :: lumen_engine :: media :: AssetKind :: Environment"
            ),
            "{tokens}"
        );
        assert!(tokens.contains("param : \"source\""), "{tokens}");
        assert!(tokens.contains("selector : Some (\"kind\")"), "{tokens}");
    }

    #[test]
    fn media_rejects_what_it_cannot_declare() {
        assert!(error(quote!(#[media(font)])).contains("unknown media kind"));
        assert!(error(quote!(#[media(image, image)])).contains("listed twice"));
        assert!(error(quote!(#[media()])).contains("at least one kind"));
        assert!(error(quote!(#[media(image, by = kind)])).contains("two or more"));
    }
}
