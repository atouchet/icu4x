// This file is part of ICU4X. For terms of use, please see the file
// called LICENSE at the top level of the ICU4X source tree
// (online at: https://github.com/unicode-org/icu4x/blob/main/LICENSE ).

use crate::utils;
use proc_macro2::Span;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Error, Ident};

pub fn derive_impl(input: &DeriveInput) -> TokenStream2 {
    if !utils::has_valid_repr(&input.attrs, |r| r == "packed" || r == "transparent") {
        return Error::new(
            input.span(),
            "derive(VarULE) must be applied to a #[repr(packed)] or #[repr(transparent)] type",
        )
        .to_compile_error();
    }
    if input.generics.type_params().next().is_some()
        || input.generics.lifetimes().next().is_some()
        || input.generics.const_params().next().is_some()
    {
        return Error::new(
            input.generics.span(),
            "derive(VarULE) must be applied to a struct without any generics",
        )
        .to_compile_error();
    }
    let struc = if let Data::Struct(ref s) = input.data {
        if s.fields.iter().next().is_none() {
            return Error::new(
                input.span(),
                "derive(VarULE) must be applied to a non-empty struct",
            )
            .to_compile_error();
        }
        s
    } else {
        return Error::new(input.span(), "derive(VarULE) must be applied to a struct")
            .to_compile_error();
    };

    let n_fields = struc.fields.len();

    let sizes = struc.fields.iter().take(n_fields - 1).map(|f| {
        let ty = &f.ty;
        quote!(::core::mem::size_of::<#ty>())
    });

    let (validators, remaining_offset) = if n_fields > 1 {
        // generate ULE validators
        crate::ule::generate_ule_validators(struc.fields.iter().take(n_fields - 1))
    } else {
        // no ULE subfields
        (
            quote!(const ZERO: usize = 0),
            Ident::new("ZERO", Span::call_site()),
        )
    };

    let unsized_field = &struc
        .fields
        .iter()
        .next_back()
        .expect("Already verified that struct is not empty")
        .ty;

    let name = &input.ident;
    let ule_size = Ident::new(
        &format!("__IMPL_VarULE_FOR_{name}_ULE_SIZE"),
        Span::call_site(),
    );

    // Safety (based on the safety checklist on the ULE trait):
    //  1. #name does not include any uninitialized or padding bytes
    //     (achieved by enforcing #[repr(transparent)] or #[repr(packed)] on a struct of only ULE types)
    //  2. #name is aligned to 1 byte.
    //     (achieved by enforcing #[repr(transparent)] or #[repr(packed)] on a struct of only ULE types)
    //  3. The impl of `validate_byte_slice()` returns an error if any byte is not valid.
    //  4. The impl of `validate_byte_slice()` returns an error if the slice cannot be used in its entirety
    //  5. The impl of `from_byte_slice_unchecked()` returns a reference to the same data.
    //  6. The other VarULE methods use the default impl
    //  7. [This impl does not enforce the non-safety equality constraint, it is up to the user to do so, ideally via a custom derive]
    quote! {
        // The size of the ULE section of this type
        const #ule_size: usize = 0 #(+ #sizes)*;
        unsafe impl zerovec::ule::VarULE for #name {
            #[inline]
            fn validate_byte_slice(bytes: &[u8]) -> Result<(), zerovec::ZeroVecError> {

                if bytes.len() < #ule_size {
                    return Err(zerovec::ZeroVecError::parse::<Self>());
                }
                #validators
                debug_assert_eq!(#remaining_offset, #ule_size);
                <#unsized_field as zerovec::ule::VarULE>::validate_byte_slice(&bytes[#remaining_offset..])?;
                Ok(())
            }
            #[inline]
            unsafe fn from_byte_slice_unchecked(bytes: &[u8]) -> &Self {
                // just the unsized part
                let unsized_bytes = &bytes[#ule_size..];
                let unsized_ref = <#unsized_field as zerovec::ule::VarULE>::from_byte_slice_unchecked(unsized_bytes);
                // We should use the pointer metadata APIs here when they are stable: https://github.com/rust-lang/rust/issues/81513
                // For now we rely on all DST metadata being a usize to extract it via a fake slice pointer
                let (_ptr, metadata): (usize, usize) = ::core::mem::transmute(unsized_ref);
                let entire_struct_as_slice: *const [u8] = ::core::slice::from_raw_parts(bytes.as_ptr(), metadata);
                &*(entire_struct_as_slice as *const Self)
            }
        }
    }
}
