//! Compile-time generation of zplugin v1 interfaces.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as Tokens;
use quote::{format_ident, quote};
use syn::{
    Attribute, Error, FnArg, GenericArgument, Ident, ItemTrait, LitInt, LitStr, Pat, PathArguments,
    ReturnType, TraitItem, TraitItemFn, Type, parse::Parse, parse::ParseStream, parse_macro_input,
};

/// Define a frozen interface, its export adapter, and its typed host proxy.
#[proc_macro_attribute]
pub fn interface(attributes: TokenStream, item: TokenStream) -> TokenStream {
    let arguments = parse_macro_input!(attributes as InterfaceArguments);
    let interface = parse_macro_input!(item as ItemTrait);
    expand(arguments, interface)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

struct InterfaceArguments {
    id: LitStr,
    version: u32,
}

impl Parse for InterfaceArguments {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut id = None;
        let mut version = None;
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<syn::Token![=]>()?;
            match key.to_string().as_str() {
                "id" if id.is_none() => id = Some(input.parse::<LitStr>()?),
                "version" if version.is_none() => {
                    let literal = input.parse::<LitInt>()?;
                    let value = literal.base10_parse::<u32>()?;
                    if value == 0 {
                        return Err(Error::new_spanned(
                            literal,
                            "interface version must be nonzero",
                        ));
                    }
                    version = Some(value);
                }
                "id" | "version" => {
                    return Err(Error::new_spanned(key, "duplicate interface option"));
                }
                _ => return Err(Error::new_spanned(key, "expected `id` or `version`")),
            }
            if !input.is_empty() {
                input.parse::<syn::Token![,]>()?;
            }
        }
        let id = id.ok_or_else(|| input.error("missing `id = \"interface.name\"`"))?;
        if id.value().is_empty()
            || id.value().len() > 255
            || !id
                .value()
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
        {
            return Err(Error::new_spanned(
                id,
                "interface ID must contain 1..=255 ASCII letters, digits, or . _ : / -",
            ));
        }
        Ok(Self {
            id,
            version: version.ok_or_else(|| input.error("missing `version = 1`"))?,
        })
    }
}

#[derive(Clone)]
enum WireType {
    Scalar(Ident),
    Bytes,
    BytesMut,
    Str,
    Unit,
    Owned,
}

impl WireType {
    fn parse(ty: &Type, output: bool) -> syn::Result<Self> {
        if let Type::Tuple(tuple) = ty {
            if output && tuple.elems.is_empty() {
                return Ok(Self::Unit);
            }
        }
        if let Type::Path(path) = ty {
            if path.qself.is_none()
                && path.path.segments.len() == 1
                && matches!(path.path.segments[0].arguments, PathArguments::None)
            {
                let ident = &path.path.segments[0].ident;
                if matches!(
                    ident.to_string().as_str(),
                    "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" | "f32" | "f64"
                ) {
                    return Ok(Self::Scalar(ident.clone()));
                }
            }
            if output && is_named_path(path, "OwnedBytes") {
                return Ok(Self::Owned);
            }
        }
        if !output {
            if let Type::Reference(reference) = ty {
                if reference.lifetime.is_some() {
                    return Err(Error::new_spanned(
                        ty,
                        "explicit boundary lifetimes are unsupported; inputs live for the current call",
                    ));
                }
                match reference.elem.as_ref() {
                    Type::Slice(slice) if is_primitive(&slice.elem, "u8") => {
                        return Ok(if reference.mutability.is_some() {
                            Self::BytesMut
                        } else {
                            Self::Bytes
                        });
                    }
                    inner if is_primitive(inner, "str") && reference.mutability.is_none() => {
                        return Ok(Self::Str);
                    }
                    _ => {}
                }
            }
        }
        Err(Error::new_spanned(
            ty,
            if output {
                "unsupported zplugin return type; use (), fixed-width scalars, or zplugin::OwnedBytes inside Result"
            } else {
                "unsupported zplugin parameter type; use fixed-width scalars, &[u8], &mut [u8], or &str"
            },
        ))
    }

    fn canonical(&self) -> String {
        match self {
            Self::Scalar(ident) => ident.to_string(),
            Self::Bytes => "bytes".into(),
            Self::BytesMut => "bytes_mut".into(),
            Self::Str => "utf8".into(),
            Self::Unit => "unit".into(),
            Self::Owned => "owned_bytes_v1".into(),
        }
    }

    fn wire(&self) -> Tokens {
        match self {
            Self::Scalar(ident) => quote!(::core::primitive::#ident),
            Self::Bytes | Self::Str => quote!(::zplugin::abi::Bytes),
            Self::BytesMut => quote!(::zplugin::abi::BytesMut),
            Self::Unit => quote!(()),
            Self::Owned => quote!(::zplugin::abi::RawBuffer),
        }
    }

    fn host_type(&self) -> Tokens {
        match self {
            Self::Scalar(ident) => quote!(::core::primitive::#ident),
            Self::Bytes => quote!(&[::core::primitive::u8]),
            Self::BytesMut => quote!(&mut [::core::primitive::u8]),
            Self::Str => quote!(&::core::primitive::str),
            Self::Unit => quote!(()),
            Self::Owned => quote!(::zplugin::Buffer<'__zplugin_lib>),
        }
    }

    fn encode(&self, name: &Ident) -> Tokens {
        match self {
            Self::Scalar(_) => quote!(#name),
            Self::Bytes | Self::Str => {
                quote!(::zplugin::abi::Bytes { ptr: #name.as_ptr(), len: #name.len() })
            }
            Self::BytesMut => {
                quote!(::zplugin::abi::BytesMut { ptr: #name.as_mut_ptr(), len: #name.len() })
            }
            _ => unreachable!("outputs cannot be arguments"),
        }
    }

    fn decode(&self, field: &Ident) -> Tokens {
        match self {
            Self::Scalar(_) => quote!(__zplugin_arguments.#field),
            Self::Bytes => quote!(unsafe { __zplugin_arguments.#field.as_slice()? }),
            Self::BytesMut => quote!(unsafe { __zplugin_arguments.#field.as_mut_slice()? }),
            Self::Str => quote!(::core::str::from_utf8(unsafe {
                __zplugin_arguments.#field.as_slice()?
            }).map_err(|_| ::zplugin::Error::InvalidUtf8)?),
            _ => unreachable!("outputs cannot be arguments"),
        }
    }
}

fn is_primitive(ty: &Type, name: &str) -> bool {
    matches!(ty, Type::Path(path) if path.qself.is_none() && path.path.is_ident(name))
}

fn is_named_path(path: &syn::TypePath, name: &str) -> bool {
    if path.qself.is_some()
        || path
            .path
            .segments
            .iter()
            .any(|segment| !matches!(segment.arguments, PathArguments::None))
    {
        return false;
    }
    let segments: Vec<_> = path
        .path
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect();
    segments == [name] || segments == ["zplugin", name]
}

fn reject_conditional(attributes: &[Attribute]) -> syn::Result<()> {
    for attr in attributes {
        if !attr.path().is_ident("doc") && !attr.path().is_ident("allow") {
            return Err(Error::new_spanned(
                attr,
                "zplugin interfaces allow only doc/allow attributes; conditional or transforming attributes can change the frozen ABI",
            ));
        }
    }
    Ok(())
}

struct Method {
    item: TraitItemFn,
    inputs: Vec<(Ident, WireType)>,
    output: WireType,
    signature: LitStr,
}

impl Method {
    fn parse(item: &TraitItemFn) -> syn::Result<Self> {
        let signature = &item.sig;
        reject_conditional(&item.attrs)?;
        if signature.asyncness.is_some()
            || signature.unsafety.is_some()
            || signature.constness.is_some()
            || signature.abi.is_some()
            || signature.variadic.is_some()
            || !signature.generics.params.is_empty()
            || signature.generics.where_clause.is_some()
            || item.default.is_some()
        {
            return Err(Error::new_spanned(
                signature,
                "zplugin methods must be synchronous, safe, nongeneric required methods without an explicit ABI",
            ));
        }
        let mut arguments = signature.inputs.iter();
        let receiver = match arguments.next() {
            Some(FnArg::Receiver(receiver))
                if receiver.reference.is_some()
                    && receiver
                        .reference
                        .as_ref()
                        .is_some_and(|(_, lifetime)| lifetime.is_none())
                    && receiver.colon_token.is_none() =>
            {
                receiver
            }
            _ => {
                return Err(Error::new_spanned(
                    signature,
                    "zplugin methods require &self or &mut self",
                ));
            }
        };
        reject_conditional(&receiver.attrs)?;
        let mut inputs = Vec::new();
        for argument in arguments {
            let FnArg::Typed(argument) = argument else {
                return Err(Error::new_spanned(argument, "unexpected method receiver"));
            };
            reject_conditional(&argument.attrs)?;
            let Pat::Ident(pattern) = argument.pat.as_ref() else {
                return Err(Error::new_spanned(
                    &argument.pat,
                    "zplugin parameters require plain identifiers",
                ));
            };
            if pattern.by_ref.is_some() || pattern.subpat.is_some() || pattern.mutability.is_some()
            {
                return Err(Error::new_spanned(
                    pattern,
                    "zplugin parameters require plain identifiers",
                ));
            }
            inputs.push((pattern.ident.clone(), WireType::parse(&argument.ty, false)?));
        }
        let ReturnType::Type(_, result_type) = &signature.output else {
            return Err(Error::new_spanned(
                signature,
                "zplugin methods must return Result<T, zplugin::Error>",
            ));
        };
        let Type::Path(result) = result_type.as_ref() else {
            return Err(Error::new_spanned(
                result_type,
                "zplugin methods must return Result<T, zplugin::Error>",
            ));
        };
        let result_path: Vec<_> = result
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        if result.qself.is_some()
            || !(result_path == ["Result"] || result_path == ["zplugin", "Result"])
            || result
                .path
                .segments
                .iter()
                .take(result.path.segments.len() - 1)
                .any(|segment| !matches!(segment.arguments, PathArguments::None))
        {
            return Err(Error::new_spanned(
                result_type,
                "zplugin methods must return Result<T, zplugin::Error>",
            ));
        }
        let PathArguments::AngleBracketed(parameters) =
            &result.path.segments.last().unwrap().arguments
        else {
            return Err(Error::new_spanned(
                result_type,
                "Result requires an explicit supported return type",
            ));
        };
        let valid_arity = if result_path == ["zplugin", "Result"] {
            1
        } else {
            2
        };
        if parameters.args.len() != valid_arity {
            return Err(Error::new_spanned(
                parameters,
                "use Result<T, zplugin::Error> or zplugin::Result<T>",
            ));
        }
        let Some(GenericArgument::Type(output_type)) = parameters.args.first() else {
            return Err(Error::new_spanned(
                parameters,
                "Result requires a type argument",
            ));
        };
        if valid_arity == 2 {
            match &parameters.args[1] {
                GenericArgument::Type(Type::Path(error)) if is_named_path(error, "Error") => {}
                error => {
                    return Err(Error::new_spanned(
                        error,
                        "zplugin errors must use zplugin::Error",
                    ));
                }
            }
        }
        let output = WireType::parse(output_type, true)?;
        let canonical = format!(
            "v1:{}:{}({})->result<{}>",
            signature.ident.to_string().trim_start_matches("r#"),
            if receiver.mutability.is_some() {
                "mut"
            } else {
                "shared"
            },
            inputs
                .iter()
                .map(|(_, input)| input.canonical())
                .collect::<Vec<_>>()
                .join(","),
            output.canonical(),
        );
        Ok(Self {
            item: item.clone(),
            inputs,
            output,
            signature: LitStr::new(&canonical, signature.ident.span()),
        })
    }
}

fn expand(arguments: InterfaceArguments, interface: ItemTrait) -> syn::Result<Tokens> {
    reject_conditional(&interface.attrs)?;
    if !interface.generics.params.is_empty()
        || interface.generics.where_clause.is_some()
        || !interface.supertraits.is_empty()
        || interface.unsafety.is_some()
        || interface.auto_token.is_some()
    {
        return Err(Error::new_spanned(
            &interface,
            "zplugin interfaces must be safe nongeneric traits without supertraits",
        ));
    }
    if interface.items.is_empty() {
        return Err(Error::new_spanned(
            &interface,
            "zplugin interfaces require at least one method",
        ));
    }
    let methods = interface.items.iter().map(|item| {
        match item {
            TraitItem::Fn(method) => Method::parse(method),
            _ => Err(Error::new_spanned(item, "zplugin interfaces support methods only; associated types and constants are unsupported")),
        }
    }).collect::<syn::Result<Vec<_>>>()?;
    let trait_name = &interface.ident;
    let visibility = &interface.vis;
    let api = format_ident!("{}Api", trait_name);
    let adapter = format_ident!("{}Export", trait_name);
    let binding = format_ident!("{}Binding", trait_name);
    let proxy = format_ident!("{}Proxy", trait_name);
    let wire_module = format_ident!("__zplugin_{}", trait_name);
    let id = arguments.id;
    let version = arguments.version;
    let signatures: Vec<_> = methods.iter().map(|method| &method.signature).collect();
    let mut wire_structs = Vec::new();
    let mut adapter_methods = Vec::new();
    let mut method_descriptors = Vec::new();
    let mut proxy_fields = Vec::new();
    let mut proxy_init = Vec::new();
    let mut proxy_methods = Vec::new();
    for (index, method) in methods.iter().enumerate() {
        let name = &method.item.sig.ident;
        let wire_name = format_ident!("Args_{}", name);
        let call_name = format_ident!("__call_{}", name);
        let field = format_ident!("__method_{}", name);
        let signature = &method.signature;
        let wire_fields: Vec<_> = method
            .inputs
            .iter()
            .enumerate()
            .map(|(i, _)| format_ident!("arg_{i}"))
            .collect();
        let wire_types: Vec<_> = method.inputs.iter().map(|(_, ty)| ty.wire()).collect();
        wire_structs.push(quote! {
            #[repr(C)]
            pub(super) struct #wire_name {
                #(pub(super) #wire_fields: #wire_types,)*
            }
        });
        let borrows: Vec<_> = method
            .inputs
            .iter()
            .zip(&wire_fields)
            .filter_map(|((_, ty), field)| {
                let mutable = matches!(ty, WireType::BytesMut);
                matches!(ty, WireType::Bytes | WireType::BytesMut | WireType::Str).then(|| {
                    quote! {
                        ::zplugin::export::Borrow {
                            ptr: __zplugin_arguments.#field.ptr as *const ::core::primitive::u8,
                            len: __zplugin_arguments.#field.len,
                            mutable: #mutable,
                        }
                    }
                })
            })
            .collect();
        let passed: Vec<_> = method
            .inputs
            .iter()
            .zip(&wire_fields)
            .map(|((_, ty), field)| ty.decode(field))
            .collect();
        let output_wire = method.output.wire();
        let validate_out = if matches!(method.output, WireType::Unit) {
            quote!()
        } else {
            quote!(::zplugin::export::check_pointer::<#output_wire>(out.cast())?;)
        };
        let write_output = match method.output {
            WireType::Unit => quote!(let _ = __zplugin_value;),
            WireType::Owned => quote! {
                // This assertion prevents a same-named user type from supplying
                // an arbitrary raw buffer through a safe `into_raw` method.
                let __zplugin_value: ::zplugin::OwnedBytes = __zplugin_value;
                unsafe { out.cast::<#output_wire>().write(__zplugin_value.into_raw()); }
            },
            _ => quote!(unsafe { out.cast::<#output_wire>().write(__zplugin_value); }),
        };
        adapter_methods.push(quote! {
            unsafe extern "C" fn #call_name(
                object: *mut ::core::ffi::c_void,
                args: *const ::core::ffi::c_void,
                out: *mut ::core::ffi::c_void,
            ) -> ::core::primitive::i32 {
                ::zplugin::export::guard(|| {
                    ::zplugin::export::check_pointer::<#wire_module::#wire_name>(args.cast())?;
                    #validate_out
                    // SAFETY: the raw ABI caller supplies readable, aligned argument storage.
                    let __zplugin_arguments = unsafe { args.cast::<#wire_module::#wire_name>().read() };
                    ::zplugin::export::validate_borrows(&[
                        ::zplugin::export::Borrow {
                            ptr: args.cast(),
                            len: ::core::mem::size_of::<#wire_module::#wire_name>(),
                            mutable: false,
                        },
                        ::zplugin::export::Borrow {
                            ptr: out.cast(),
                            len: ::core::mem::size_of::<#output_wire>(),
                            mutable: true,
                        },
                        #(#borrows,)*
                    ])?;
                    // SAFETY: the instance was created by this adapter; ranges are checked
                    // before forming references and with_object enforces nonreentrancy.
                    let __zplugin_value = unsafe {
                        ::zplugin::export::with_object::<__ZpluginImplementation, _>(object, |implementation| {
                            <__ZpluginImplementation as #trait_name>::#name(implementation, #(#passed),*)
                        })
                    }?;
                    #write_output
                    Ok(())
                })
            }
        });
        method_descriptors.push(quote! {
            ::zplugin::abi::Method {
                signature: ::zplugin::abi::Bytes::from_static(#signature.as_bytes()),
                call: Some(Self::#call_name),
            }
        });
        proxy_fields.push(quote!(#field: ::zplugin::abi::Call));
        proxy_init.push(quote!(#field: instance.method(#index)));
        let parameter_names: Vec<_> = method.inputs.iter().map(|(name, _)| name).collect();
        let parameter_types: Vec<_> = method.inputs.iter().map(|(_, ty)| ty.host_type()).collect();
        let values: Vec<_> = method
            .inputs
            .iter()
            .map(|(name, ty)| ty.encode(name))
            .collect();
        let host_output = method.output.host_type();
        let host_return = if matches!(method.output, WireType::Owned) {
            quote!(
                // SAFETY: a successful call transfers one fresh owned result
                // from this instance's plugin; buffer retains the matching lease.
                unsafe { self.__instance.buffer(__zplugin_output.assume_init()) }
            )
        } else {
            quote!(Ok(unsafe { __zplugin_output.assume_init() }))
        };
        let docs: Vec<_> = method
            .item
            .attrs
            .iter()
            .filter(|attr| attr.path().is_ident("doc"))
            .collect();
        proxy_methods.push(quote! {
            #(#docs)*
            #[inline]
            pub fn #name(&mut self, #(#parameter_names: #parameter_types),*) -> ::zplugin::Result<#host_output> {
                let __zplugin_arguments = #wire_module::#wire_name { #(#wire_fields: #values,)* };
                let mut __zplugin_output = ::core::mem::MaybeUninit::<#output_wire>::uninit();
                // SAFETY: the checked binding retains its library and method signature;
                // Rust borrows keep all arguments valid and disjoint throughout the call.
                let __zplugin_status = unsafe {
                    (self.#field)(
                        self.__instance.handle(),
                        (&__zplugin_arguments as *const #wire_module::#wire_name).cast(),
                        __zplugin_output.as_mut_ptr().cast(),
                    )
                };
                ::zplugin::status_result(__zplugin_status)?;
                #host_return
            }
        });
    }
    Ok(quote! {
        #interface

        #[doc(hidden)]
        #[allow(non_snake_case, non_camel_case_types)]
        mod #wire_module {
            #(#wire_structs)*
        }

        #[doc = "The frozen interface descriptor and typed proxy constructors."]
        #visibility struct #api;

        #[doc = "A validated, borrowed interface binding. Create objects without rebinding."]
        #visibility struct #binding<'__zplugin_lib> {
            __binding: ::zplugin::Binding<'__zplugin_lib>,
        }

        #[doc = "An object that retains its plugin and caches its validated method table."]
        #visibility struct #proxy<'__zplugin_lib> {
            __instance: ::zplugin::Instance<'__zplugin_lib>,
            #(#proxy_fields,)*
        }

        #[doc = "The generated plugin-side adapter. Use with zplugin::export!."]
        #visibility struct #adapter<__ZpluginImplementation>(
            ::core::marker::PhantomData<__ZpluginImplementation>
        );

        impl #api {
            /// Exact interface ID, version, method order, and method signatures.
            pub const SPEC: ::zplugin::InterfaceSpec = ::zplugin::InterfaceSpec {
                id: #id,
                version: #version,
                signatures: &[#(#signatures,)*],
            };

            /// Validate the interface once; reuse the binding to create objects.
            pub fn bind(plugin: &::zplugin::Plugin) -> ::zplugin::Result<#binding<'_>> {
                Ok(#binding { __binding: plugin.bind(Self::SPEC)? })
            }

            /// Bind and create a borrowed object.
            pub fn create(plugin: &::zplugin::Plugin) -> ::zplugin::Result<#proxy<'_>> {
                Self::bind(plugin)?.create()
            }

            /// Type-check an instance, including one created with an owned library lease.
            pub fn wrap<'__zplugin_lib>(instance: ::zplugin::Instance<'__zplugin_lib>)
                -> ::zplugin::Result<#proxy<'__zplugin_lib>>
            {
                instance.check(Self::SPEC)?;
                Ok(#proxy {
                    #(#proxy_init,)*
                    __instance: instance,
                })
            }
        }

        impl<'__zplugin_lib> #binding<'__zplugin_lib> {
            /// Create another plugin-owned instance using the cached binding.
            pub fn create(&self) -> ::zplugin::Result<#proxy<'__zplugin_lib>> {
                #api::wrap(self.__binding.create()?)
            }
        }

        impl<'__zplugin_lib> #proxy<'__zplugin_lib> {
            #(#proxy_methods)*
        }

        impl<__ZpluginImplementation: #trait_name + 'static> #adapter<__ZpluginImplementation> {
            #(#adapter_methods)*
        }

        // SAFETY: each adapter uses its canonical v1 representation and validates
        // borrowed ranges before constructing Rust references.
        unsafe impl<__ZpluginImplementation: #trait_name + 'static> ::zplugin::export::ExportInterface
            for #adapter<__ZpluginImplementation>
        {
            type Implementation = __ZpluginImplementation;
            const ID: &'static str = #id;
            const VERSION: u32 = #version;
            const METHODS: &'static [::zplugin::abi::Method] = &[#(#method_descriptors,)*];
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expansion(source: &str) -> syn::Result<Tokens> {
        expand(
            syn::parse_str("id = \"tests.example\", version = 1")?,
            syn::parse_str(source)?,
        )
    }

    #[test]
    fn accepts_all_wire_types() {
        let tokens = expansion(
            "pub trait Example {
                fn compute(&self, a: u64, b: f32) -> Result<i64, zplugin::Error>;
                fn copy(&mut self, input: &[u8], output: &mut [u8], label: &str) -> zplugin::Result<u32>;
                fn unit(&mut self) -> zplugin::Result<()>;
                fn owned(&mut self) -> zplugin::Result<zplugin::OwnedBytes>;
            }",
        ).unwrap();
        syn::parse2::<syn::File>(tokens).unwrap();
    }

    #[test]
    fn argument_names_do_not_change_abi() {
        let parse =
            |source| Method::parse(&syn::parse_str::<TraitItemFn>(source).unwrap()).unwrap();
        assert_eq!(
            parse("fn calculate(&self, left:u64)->zplugin::Result<u32>;")
                .signature
                .value(),
            parse("fn calculate(&self, renamed:u64)->zplugin::Result<u32>;")
                .signature
                .value(),
        );
    }

    #[test]
    fn receiver_and_order_are_part_of_signature() {
        let a = Method::parse(
            &syn::parse_str("fn f(&self,a:u64,b:u32)->zplugin::Result<()>;").unwrap(),
        )
        .unwrap();
        let b = Method::parse(
            &syn::parse_str("fn f(&mut self,a:u64,b:u32)->zplugin::Result<()>;").unwrap(),
        )
        .unwrap();
        let c = Method::parse(
            &syn::parse_str("fn f(&self,a:u32,b:u64)->zplugin::Result<()>;").unwrap(),
        )
        .unwrap();
        assert_ne!(a.signature.value(), b.signature.value());
        assert_ne!(a.signature.value(), c.signature.value());
    }

    #[test]
    fn rejects_unsupported_interfaces() {
        for source in [
            "trait Example<T> { fn f(&self)->zplugin::Result<()>; }",
            "trait Example: Send { fn f(&self)->zplugin::Result<()>; }",
            "trait Example { type Output; }",
            "trait Example { fn f(&self, value:String)->zplugin::Result<()>; }",
            "trait Example { fn f(&self, value:usize)->zplugin::Result<()>; }",
            "trait Example { fn f(&self)->u32; }",
            "trait Example { async fn f(&self)->zplugin::Result<()>; }",
            "trait Example { #[cfg(unix)] fn f(&self)->zplugin::Result<()>; }",
            "trait Example { fn f(&self)->Result<u32,OtherError>; }",
            "trait Example { fn f(&self)->zplugin::Result<&str>; }",
        ] {
            assert!(
                expansion(source).is_err(),
                "unexpected acceptance: {source}"
            );
        }
    }

    #[test]
    fn validates_options() {
        for source in [
            "id=\"\",version=1",
            "id=\"test\",version=0",
            "id=\"test\"",
            "version=1",
            "id=\"test\",version=1,version=2",
            "id=\"with space\",version=1",
        ] {
            assert!(
                syn::parse_str::<InterfaceArguments>(source).is_err(),
                "accepted {source}"
            );
        }
    }
}
