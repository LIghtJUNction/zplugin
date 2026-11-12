use super::{InterfaceArguments, reject_conditional};
use proc_macro2::TokenStream as Tokens;
use quote::{format_ident, quote};
use syn::{Error, Expr, Fields, Ident, Item, Lit, LitStr, Member, Type};

pub(super) fn concat(parts: &[Tokens]) -> Tokens {
    quote! {
        // SAFETY: concatenating complete UTF-8 strings preserves UTF-8 validity.
        {
            const __ZPLUGIN_SCHEMA_BYTES: [u8; 0 #( + (#parts).len() )*] =
                ::zplugin::value::concat(&[#(#parts),*]);
            unsafe { ::core::str::from_utf8_unchecked(&__ZPLUGIN_SCHEMA_BYTES) }
        }
    }
}

pub(super) fn value_type(ty: &Type) -> syn::Result<Tokens> {
    let Type::Path(path) = ty else {
        return Err(Error::new_spanned(
            ty,
            "ABI values require fixed-width scalars or named #[zplugin::abi_type] values",
        ));
    };
    if path.qself.is_some()
        || path
            .path
            .segments
            .iter()
            .any(|s| !matches!(s.arguments, syn::PathArguments::None))
    {
        return Err(Error::new_spanned(
            ty,
            "generic and owning Rust types are not stable ABI values",
        ));
    }
    let ident = &path.path.segments.last().unwrap().ident;
    if matches!(
        ident.to_string().as_str(),
        "bool"
            | "char"
            | "usize"
            | "isize"
            | "u128"
            | "i128"
            | "String"
            | "Vec"
            | "Box"
            | "Arc"
            | "Rc"
            | "Self"
            | "OwnedBytes"
            | "OwnedResource"
            | "Callback"
    ) {
        return Err(Error::new_spanned(
            ty,
            "unsupported ABI value type; use fixed-width scalars or named #[zplugin::abi_type] values",
        ));
    }
    if path.path.segments.len() == 1
        && matches!(
            ident.to_string().as_str(),
            "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" | "f32" | "f64"
        )
    {
        return Ok(quote!(::core::primitive::#ident));
    }
    Ok(quote!(#ty))
}

pub(super) fn nested_type(ty: &Type) -> syn::Result<Tokens> {
    let checked = value_type(ty)?;
    if let Type::Path(path) = ty {
        if path.path.leading_colon.is_none()
            && path
                .path
                .segments
                .first()
                .is_some_and(|s| s.ident == "super" || s.ident == "self")
        {
            return Ok(quote!(super::#ty));
        }
    }
    Ok(checked)
}

fn c_identifier(ident: &Ident) -> syn::Result<String> {
    let name = ident.to_string().trim_start_matches("r#").to_owned();
    if name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
        Ok(name)
    } else {
        Err(Error::new_spanned(
            ident,
            "ABI field and variant names must use ASCII identifiers for C/C++/Zig generation",
        ))
    }
}

struct Field {
    member: Member,
    local: Ident,
    c_name: String,
    ty: Tokens,
}

fn fields(source: &Fields) -> syn::Result<Vec<Field>> {
    source
        .iter()
        .enumerate()
        .map(|(index, field)| {
            reject_conditional(&field.attrs)?;
            let c_name = if let Some(ident) = &field.ident {
                c_identifier(ident)?
            } else {
                format!("arg_{index}")
            };
            if matches!(
                c_name.as_str(),
                "auto"
                    | "break"
                    | "case"
                    | "char"
                    | "const"
                    | "continue"
                    | "default"
                    | "do"
                    | "double"
                    | "else"
                    | "enum"
                    | "extern"
                    | "float"
                    | "for"
                    | "goto"
                    | "if"
                    | "inline"
                    | "int"
                    | "long"
                    | "register"
                    | "restrict"
                    | "return"
                    | "short"
                    | "signed"
                    | "sizeof"
                    | "static"
                    | "struct"
                    | "switch"
                    | "typedef"
                    | "union"
                    | "unsigned"
                    | "void"
                    | "volatile"
                    | "while"
                    | "class"
                    | "template"
                    | "typename"
                    | "new"
                    | "delete"
                    | "operator"
                    | "private"
                    | "protected"
                    | "public"
                    | "virtual"
                    | "friend"
                    | "namespace"
                    | "using"
                    | "bool"
                    | "true"
                    | "false"
                    | "this"
                    | "throw"
                    | "try"
                    | "catch"
                    | "alignas"
                    | "alignof"
                    | "constexpr"
                    | "consteval"
                    | "constinit"
                    | "nullptr"
                    | "noexcept"
                    | "requires"
                    | "concept"
                    | "co_await"
                    | "co_return"
                    | "co_yield"
            ) {
                return Err(Error::new_spanned(
                    field,
                    "ABI field name is reserved in C/C++; rename this field",
                ));
            }
            Ok(Field {
                member: field
                    .ident
                    .clone()
                    .map(Member::Named)
                    .unwrap_or_else(|| Member::Unnamed(index.into())),
                local: format_ident!("field_{index}"),
                c_name,
                ty: nested_type(&field.ty)?,
            })
        })
        .collect()
}

fn wire_fields(fields: &[Field]) -> Tokens {
    let names: Vec<_> = fields.iter().map(|f| &f.local).collect();
    let types: Vec<_> = fields.iter().map(|f| &f.ty).collect();
    if fields.is_empty() {
        quote!(_unused: ::core::primitive::u8)
    } else {
        quote!(#(#names: <#types as ::zplugin::AbiValue>::Wire),*)
    }
}

fn schema_fields(fields: &[Field], parts: &mut Vec<Tokens>) {
    for (index, field) in fields.iter().enumerate() {
        let name = LitStr::new(
            &format!("{}{}:", if index == 0 { "" } else { "," }, field.c_name),
            proc_macro2::Span::call_site(),
        );
        let ty = &field.ty;
        parts.push(quote!(#name));
        parts.push(quote!(<#ty as ::zplugin::AbiValue>::SCHEMA));
    }
}

fn write_dependencies(fields: &[Field]) -> Tokens {
    let types: Vec<_> = fields.iter().map(|f| &f.ty).collect();
    quote!(#(<#types as ::zplugin::AbiValue>::write_c(writer)?;)*)
}

fn write_fields(fields: &[Field]) -> Tokens {
    if fields.is_empty() {
        return quote!(writer.write_str("    uint8_t _unused;\n")?;);
    }
    let names: Vec<_> = fields.iter().map(|f| &f.c_name).collect();
    let types: Vec<_> = fields.iter().map(|f| &f.ty).collect();
    quote!(#(::core::writeln!(writer, "    {} {};", <#types as ::zplugin::AbiValue>::C_TYPE, #names)?;)*)
}

fn attributes(item: &Item) -> syn::Result<()> {
    let attrs = match item {
        Item::Struct(i) => &i.attrs,
        Item::Enum(i) => &i.attrs,
        _ => {
            return Err(Error::new_spanned(
                item,
                "abi_type supports named structs and enums only",
            ));
        }
    };
    for attribute in attrs {
        if !["doc", "allow", "derive", "repr", "non_exhaustive"]
            .iter()
            .any(|name| attribute.path().is_ident(name))
        {
            return Err(Error::new_spanned(
                attribute,
                "ABI values allow doc/allow/derive/repr/non_exhaustive attributes only; conditional fields change the frozen schema",
            ));
        }
    }
    Ok(())
}

fn c_name(arguments: &InterfaceArguments) -> String {
    let id: String = arguments
        .id
        .value()
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() {
                (byte as char).to_string()
            } else {
                format!("_{byte:02x}")
            }
        })
        .collect();
    format!("zp_{id}_v{}", arguments.version)
}

pub(super) fn expand(arguments: InterfaceArguments, item: Item) -> syn::Result<Tokens> {
    attributes(&item)?;
    let c_type = c_name(&arguments);
    let guard = format!("ZPLUGIN_TYPE_{c_type}_DEFINED");
    let (name, generics, body) = match &item {
        Item::Struct(source) => {
            if !matches!(source.fields, Fields::Named(_)) || source.fields.is_empty() {
                return Err(Error::new_spanned(
                    source,
                    "ABI structs require at least one named field",
                ));
            }
            (
                &source.ident,
                &source.generics,
                structure(&arguments, &source.ident, &source.fields, &c_type, &guard)?,
            )
        }
        Item::Enum(source) => (
            &source.ident,
            &source.generics,
            enumeration(&arguments, &source.ident, &source.variants, &c_type, &guard)?,
        ),
        _ => unreachable!(),
    };
    if !generics.params.is_empty() || generics.where_clause.is_some() {
        return Err(Error::new_spanned(
            generics,
            "ABI value declarations must be nongeneric; concretize generic values before the boundary",
        ));
    }
    let module = format_ident!("__zplugin_value_{}", name);
    Ok(quote! {
        #item
        #[doc(hidden)]
        #[allow(non_snake_case, non_camel_case_types)]
        mod #module {
            #[allow(unused_imports)]
            use super::*;
            #body
        }
    })
}

fn structure(
    arguments: &InterfaceArguments,
    name: &Ident,
    source: &Fields,
    c_type: &str,
    guard: &str,
) -> syn::Result<Tokens> {
    let fields = fields(source)?;
    let wire_fields = wire_fields(&fields);
    let mut parts = vec![
        quote!("struct_v1<"),
        {
            let id = &arguments.id;
            quote!(#id)
        },
        {
            let v = arguments.version.to_string();
            quote!(#v)
        },
        quote!(">{"),
    ];
    // Separate the type ID and version without depending on Rust's source name.
    parts.insert(2, quote!("@"));
    schema_fields(&fields, &mut parts);
    parts.push(quote!("}"));
    let schema = concat(&parts);
    let names: Vec<_> = fields.iter().map(|f| &f.member).collect();
    let locals: Vec<_> = fields.iter().map(|f| &f.local).collect();
    let types: Vec<_> = fields.iter().map(|f| &f.ty).collect();
    let dependencies = write_dependencies(&fields);
    let declaration = write_fields(&fields);
    Ok(quote! {
        #[repr(C)]
        #[derive(Clone, Copy)]
        pub struct Wire { #wire_fields }
        // SAFETY: only recursively checked AbiValue wire fields are present, in
        // declaration order; conversion never exposes the Rust struct's layout.
        unsafe impl ::zplugin::AbiValue for super::#name {
            type Wire = Wire;
            const SCHEMA: &'static str = #schema;
            const C_TYPE: &'static str = #c_type;
            fn into_wire(self) -> Wire {
                Wire { #(#locals: <#types as ::zplugin::AbiValue>::into_wire(self.#names)),* }
            }
            fn from_wire(wire: Wire) -> ::zplugin::Result<Self> {
                Ok(Self { #(#names: <#types as ::zplugin::AbiValue>::from_wire(wire.#locals)?),* })
            }
            fn write_c(writer: &mut impl ::core::fmt::Write) -> ::core::fmt::Result {
                #dependencies
                ::core::writeln!(writer, "#ifndef {}\n#define {}\ntypedef struct {} {{", #guard, #guard, #c_type)?;
                #declaration
                ::core::writeln!(writer, "}} {};\n#endif", #c_type)
            }
        }
    })
}

struct Variant {
    name: Ident,
    c_name: String,
    tag: u32,
    fields: Vec<Field>,
    source: Fields,
}

fn enumeration(
    arguments: &InterfaceArguments,
    name: &Ident,
    variants: &syn::punctuated::Punctuated<syn::Variant, syn::Token![,]>,
    c_type: &str,
    guard: &str,
) -> syn::Result<Tokens> {
    if variants.is_empty() {
        return Err(Error::new_spanned(
            name,
            "ABI enums require at least one variant",
        ));
    }
    let mut tags = std::collections::BTreeSet::new();
    let mut next = Some(0u32);
    let variants: Vec<Variant> = variants
        .iter()
        .map(|variant| {
            reject_conditional(&variant.attrs)?;
            let tag = match &variant.discriminant {
                Some((_, Expr::Lit(value))) => match &value.lit {
                    Lit::Int(value) => value.base10_parse::<u32>()?,
                    _ => {
                        return Err(Error::new_spanned(
                            value,
                            "ABI enum tags must be u32 integer literals",
                        ));
                    }
                },
                Some((_, expression)) => {
                    return Err(Error::new_spanned(
                        expression,
                        "ABI enum tags must be u32 integer literals",
                    ));
                }
                None => {
                    next.ok_or_else(|| Error::new_spanned(variant, "ABI enum tag overflows u32"))?
                }
            };
            if !tags.insert(tag) {
                return Err(Error::new_spanned(variant, "duplicate ABI enum tag"));
            }
            next = tag.checked_add(1);
            Ok(Variant {
                name: variant.ident.clone(),
                c_name: c_identifier(&variant.ident)?,
                tag,
                fields: fields(&variant.fields)?,
                source: variant.fields.clone(),
            })
        })
        .collect::<syn::Result<_>>()?;
    let id = &arguments.id;
    let version = arguments.version.to_string();
    let fieldless = variants.iter().all(|v| matches!(v.source, Fields::Unit));
    let kind = if fieldless {
        "enum_u32_v1<"
    } else {
        "enum_tagged_v1<"
    };
    let mut parts = vec![
        quote!(#kind),
        quote!(#id),
        quote!("@"),
        quote!(#version),
        quote!(">{"),
    ];
    for (index, variant) in variants.iter().enumerate() {
        let intro = format!(
            "{}{}={}(",
            if index == 0 { "" } else { "," },
            variant.c_name,
            variant.tag
        );
        parts.push(quote!(#intro));
        schema_fields(&variant.fields, &mut parts);
        parts.push(quote!(")"));
    }
    parts.push(quote!("}"));
    let schema = concat(&parts);
    let names: Vec<_> = variants.iter().map(|v| &v.name).collect();
    let tags: Vec<_> = variants.iter().map(|v| v.tag).collect();
    let c_names: Vec<_> = variants.iter().map(|v| &v.c_name).collect();
    let constants = quote!(#(::core::writeln!(writer, "#define {}_{} UINT32_C({})", #c_type, #c_names, #tags)?;)*);
    if fieldless {
        return Ok(quote! {
            // SAFETY: wire tags are plain u32 and invalid tags are rejected before
            // constructing the Rust enum; its native layout never crosses the ABI.
            unsafe impl ::zplugin::AbiValue for super::#name {
                type Wire = ::core::primitive::u32;
                const SCHEMA: &'static str = #schema;
                const C_TYPE: &'static str = #c_type;
                fn into_wire(self) -> Self::Wire { match self { #(Self::#names => #tags),* } }
                fn from_wire(wire: Self::Wire) -> ::zplugin::Result<Self> {
                    match wire { #(#tags => Ok(Self::#names),)* _ => Err(::zplugin::Error::InvalidArgument) }
                }
                fn write_c(writer: &mut impl ::core::fmt::Write) -> ::core::fmt::Result {
                    ::core::writeln!(writer, "#ifndef {}\n#define {}\ntypedef uint32_t {};", #guard, #guard, #c_type)?;
                    #constants
                    writer.write_str("#endif\n")
                }
            }
        });
    }
    let mut payloads = Vec::new();
    let mut union_fields = Vec::new();
    let mut encoded = Vec::new();
    let mut decoded = Vec::new();
    let mut c_dependencies = Vec::new();
    let mut c_payloads = Vec::new();
    let mut c_union = Vec::new();
    for (index, variant) in variants.iter().enumerate() {
        let tag = variant.tag;
        let variant_name = &variant.name;
        let payload = format_ident!("Variant_{index}");
        let union_field = format_ident!("variant_{index}");
        let fields = wire_fields(&variant.fields);
        payloads.push(quote!(#[repr(C)] #[derive(Clone, Copy)] pub struct #payload { #fields }));
        union_fields.push(quote!(#union_field: #payload));
        let locals: Vec<_> = variant.fields.iter().map(|f| &f.local).collect();
        let members: Vec<_> = variant.fields.iter().map(|f| &f.member).collect();
        let types: Vec<_> = variant.fields.iter().map(|f| &f.ty).collect();
        let pattern = match &variant.source {
            Fields::Unit => quote!(Self::#variant_name),
            Fields::Named(_) => quote!(Self::#variant_name { #(#members: #locals),* }),
            Fields::Unnamed(_) => quote!(Self::#variant_name(#(#locals),*)),
        };
        let assignment = if variant.fields.is_empty() {
            quote!(_unused:0)
        } else {
            quote!(#(#locals: <#types as ::zplugin::AbiValue>::into_wire(#locals)),*)
        };
        encoded.push(quote!(#pattern => Wire { tag:#tag, payload:Payload { #union_field:#payload { #assignment } } }));
        let reads: Vec<_> = locals
            .iter()
            .zip(&types)
            .map(|(local, ty)| quote!(<#ty as ::zplugin::AbiValue>::from_wire(value.#local)?))
            .collect();
        let constructed = match &variant.source {
            Fields::Unit => quote!(Self::#variant_name),
            Fields::Named(_) => quote!(Self::#variant_name { #(#members:#reads),* }),
            Fields::Unnamed(_) => quote!(Self::#variant_name(#(#reads),*)),
        };
        let read = if variant.fields.is_empty() {
            quote!()
        } else {
            quote!(let value = unsafe { wire.payload.#union_field };)
        };
        decoded.push(quote!(#tag => { #read Ok(#constructed) }));
        c_dependencies.push(write_dependencies(&variant.fields));
        let c_payload_name = format!("{c_type}_variant_{index}");
        let c_fields = write_fields(&variant.fields);
        c_payloads.push(quote! {
            ::core::writeln!(writer, "typedef struct {} {{", #c_payload_name)?;
            #c_fields
            ::core::writeln!(writer, "}} {};", #c_payload_name)?;
        });
        let c_union_name = format!("variant_{index}");
        c_union.push(
            quote!(::core::writeln!(writer, "        {} {};", #c_payload_name, #c_union_name)?;),
        );
    }
    Ok(quote! {
        #(#payloads)*
        #[repr(C)] #[derive(Clone, Copy)] pub union Payload { #(#union_fields),* }
        #[repr(C)] #[derive(Clone, Copy)] pub struct Wire { tag: ::core::primitive::u32, payload:Payload }
        // SAFETY: the explicit tag selects a repr(C) payload of checked fields.
        // Wire constructors are private, so safe Rust cannot forge an active tag
        // with an uninitialized payload; foreign callers initialize active fields.
        unsafe impl ::zplugin::AbiValue for super::#name {
            type Wire = Wire;
            const SCHEMA: &'static str = #schema;
            const C_TYPE: &'static str = #c_type;
            fn into_wire(self) -> Wire { match self { #(#encoded),* } }
            fn from_wire(wire: Wire) -> ::zplugin::Result<Self> {
                match wire.tag { #(#decoded,)* _ => Err(::zplugin::Error::InvalidArgument) }
            }
            fn write_c(writer: &mut impl ::core::fmt::Write) -> ::core::fmt::Result {
                #(#c_dependencies)*
                ::core::writeln!(writer, "#ifndef {}\n#define {}", #guard, #guard)?;
                #constants
                #(#c_payloads)*
                ::core::writeln!(writer, "typedef struct {} {{\n    uint32_t tag;\n    union {{", #c_type)?;
                #(#c_union)*
                ::core::writeln!(writer, "    }} payload;\n}} {};\n#endif", #c_type)
            }
        }
    })
}
