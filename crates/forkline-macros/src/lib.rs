//! The `interface!` macro: declare which request-map keys a module touches.
//!
//! ```ignore
//! forkline::interface! {
//!     pub struct JoinIo {
//!         in     svc_response: String,
//!         in     svc_time_taken: i64,
//!         inout  total_response: String,
//!         out    first: bool,
//!         unset  scratch: Vec<u8>,
//!     }
//! }
//! ```
//!
//! generates `JoinIo<'a>` with
//!
//! * `in`: `fn svc_response(&self) -> Option<&String>`; validation requires
//!   the key to be written upstream on every path
//! * `opt`: the same getter, for keys that may legitimately be absent
//! * `out`: `fn set_first(&mut self, value: bool)`
//! * `inout`: getter, setter and `fn total_response_mut(&mut self) -> Option<&mut String>`
//! * `unset`: `fn unset_scratch(&mut self)`
//!
//! and implements `Interface<'a>` with a `CONTRACT` listing every key.
//!
//! Generated code refers to `::forkline`. Crates that depend on
//! `forkline-core` directly can say so with `#[forkline(crate = forkline_core)]`
//! on the struct.

#![forbid(unsafe_code)]

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Attribute, Ident, Path, Token, Type, Visibility, braced};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    In,
    Opt,
    Out,
    InOut,
    Unset,
}

struct Field {
    attrs: Vec<Attribute>,
    access: Access,
    name: Ident,
    ty: Type,
}

impl Parse for Field {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let access = if input.peek(Token![in]) {
            input.parse::<Token![in]>()?;
            Access::In
        } else {
            let word: Ident = input.parse()?;
            match word.to_string().as_str() {
                "opt" => Access::Opt,
                "out" => Access::Out,
                "inout" => Access::InOut,
                "unset" => Access::Unset,
                _ => {
                    return Err(syn::Error::new(
                        word.span(),
                        "expected `in`, `opt`, `out`, `inout` or `unset` before the key name",
                    ));
                }
            }
        };
        let name: Ident = input.parse()?;
        input.parse::<Token![:]>()?;
        let ty: Type = input.parse()?;
        Ok(Field { attrs, access, name, ty })
    }
}

struct InterfaceDef {
    attrs: Vec<Attribute>,
    krate: Path,
    vis: Visibility,
    name: Ident,
    fields: Vec<Field>,
}

fn take_crate_attr(attrs: &mut Vec<Attribute>) -> syn::Result<Option<Path>> {
    let mut krate = None;
    let mut keep = Vec::with_capacity(attrs.len());
    for attr in attrs.drain(..) {
        if attr.path().is_ident("forkline") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("crate") {
                    krate = Some(meta.value()?.parse::<Path>()?);
                    Ok(())
                } else {
                    Err(meta.error("expected `crate = path`"))
                }
            })?;
        } else {
            keep.push(attr);
        }
    }
    *attrs = keep;
    Ok(krate)
}

impl Parse for InterfaceDef {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut attrs = input.call(Attribute::parse_outer)?;
        let krate = take_crate_attr(&mut attrs)?.unwrap_or_else(|| syn::parse_quote!(::forkline));
        let vis: Visibility = input.parse()?;
        input.parse::<Token![struct]>()?;
        let name: Ident = input.parse()?;
        let body;
        braced!(body in input);
        let fields: Punctuated<Field, Token![,]> = body.parse_terminated(Field::parse, Token![,])?;
        if !input.is_empty() {
            return Err(input.error("expected one struct per interface! call"));
        }
        Ok(InterfaceDef { attrs, krate, vis, name, fields: fields.into_iter().collect() })
    }
}

fn expand(def: InterfaceDef) -> syn::Result<TokenStream2> {
    let InterfaceDef { attrs, krate, vis, name, fields } = def;
    let rt = quote!(#krate::__rt);

    let mut seen = std::collections::HashSet::new();
    for f in &fields {
        if !seen.insert(f.name.to_string()) {
            return Err(syn::Error::new(f.name.span(), format!("key `{}` is declared twice", f.name)));
        }
    }

    let lt = syn::Lifetime::new("'__fl", Span::call_site());
    let mut specs = Vec::new();
    let mut methods = Vec::new();
    let mut asserts = Vec::new();

    for (index, f) in fields.iter().enumerate() {
        let Field { attrs, access, name: key, ty } = f;
        let key_str = key.to_string();
        let access_tok = match access {
            Access::In => quote!(#rt::Access::In),
            Access::Opt => quote!(#rt::Access::Opt),
            Access::Out => quote!(#rt::Access::Out),
            Access::InOut => quote!(#rt::Access::InOut),
            Access::Unset => quote!(#rt::Access::Unset),
        };
        specs.push(quote! {
            #rt::KeySpec {
                name: #key_str,
                access: #access_tok,
                type_id: #rt::type_id_of::<#ty>,
                type_name: #rt::type_name_of::<#ty>,
            }
        });
        asserts.push(quote! { #rt::assert_map_value::<#ty>(); });

        let set = format_ident!("set_{}", key);
        let get_mut = format_ident!("{}_mut", key);
        let unset = format_ident!("unset_{}", key);
        let doc_get = format!("The value of `{key_str}`, if present.");
        let doc_set = format!("Set `{key_str}`.");
        let doc_mut = format!("Mutable access to `{key_str}`, if present (copied first if a fork shares it).");
        let doc_unset = format!("Remove `{key_str}` from the map.");
        let getter = quote! {
            #(#attrs)*
            #[doc = #doc_get]
            #[inline]
            pub fn #key(&self) -> ::core::option::Option<&#ty> {
                let slot = #rt::slot_at(self.slots, #index)?;
                self.map.get::<#ty>(slot)
            }
        };
        let setter = quote! {
            #(#attrs)*
            #[doc = #doc_set]
            #[inline]
            pub fn #set(&mut self, value: #ty) {
                if let ::core::option::Option::Some(slot) = #rt::slot_at(self.slots, #index) {
                    self.map.set::<#ty>(slot, value);
                }
            }
        };
        match access {
            Access::In | Access::Opt => methods.push(getter),
            Access::Out => methods.push(setter),
            Access::InOut => {
                methods.push(getter);
                methods.push(setter);
                methods.push(quote! {
                    #(#attrs)*
                    #[doc = #doc_mut]
                    #[inline]
                    pub fn #get_mut(&mut self) -> ::core::option::Option<&mut #ty> {
                        let slot = #rt::slot_at(self.slots, #index)?;
                        self.map.get_mut::<#ty>(slot)
                    }
                });
            }
            Access::Unset => methods.push(quote! {
                #(#attrs)*
                #[doc = #doc_unset]
                #[inline]
                pub fn #unset(&mut self) {
                    if let ::core::option::Option::Some(slot) = #rt::slot_at(self.slots, #index) {
                        self.map.unset(slot);
                    }
                }
            }),
        }
    }

    let has_doc = attrs.iter().any(|a| a.path().is_ident("doc"));
    let default_doc = (!has_doc).then(|| {
        let text = format!("Data contract `{name}`: a typed view of the request map.");
        quote!(#[doc = #text])
    });
    Ok(quote! {
        #(#attrs)*
        #default_doc
        #vis struct #name<#lt> {
            map: &#lt mut #rt::RequestMap,
            slots: &#lt [#rt::Slot],
        }

        impl<#lt> ::core::fmt::Debug for #name<#lt> {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.debug_struct(::core::stringify!(#name)).field("slots", &self.slots).finish()
            }
        }

        const _: () = { #(#asserts)* };

        impl<#lt> #rt::Interface<#lt> for #name<#lt> {
            const CONTRACT: &'static [#rt::KeySpec] = &[#(#specs),*];

            fn bind(map: &#lt mut #rt::RequestMap, slots: &#lt [#rt::Slot]) -> Self {
                #name { map, slots }
            }
        }

        #[allow(dead_code)]
        impl<#lt> #name<#lt> {
            #(#methods)*
        }
    })
}

/// Declare a module's data contract and generate its typed accessors.
///
/// See the crate documentation for the syntax.
#[proc_macro]
pub fn interface(input: TokenStream) -> TokenStream {
    match syn::parse::<InterfaceDef>(input).and_then(expand) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}
