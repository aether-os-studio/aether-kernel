extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, ReturnType, Type, parse_macro_input};

#[proc_macro_attribute]
pub fn frame_entry(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(item as ItemFn);
    let fn_attrs = &input_fn.attrs;
    let fn_vis = &input_fn.vis;
    let fn_name = &input_fn.sig.ident;
    let fn_block = &input_fn.block;

    // 检查返回类型必须为 `!`
    match &input_fn.sig.output {
        ReturnType::Type(_, ty) => {
            if let Type::Never(_) = **ty {
                // OK
            } else {
                return quote! {
                    compile_error!("Functions marked with #[frame_entry] must return `!`");
                }
                .into();
            }
        }
        ReturnType::Default => {
            return quote! {
                compile_error!("Functions marked with #[frame_entry] must return `!`");
            }
            .into();
        }
    }

    let output = quote! {
        #(#fn_attrs)*
        #fn_vis fn #fn_name() -> ! {
            #fn_block
        }

        #[unsafe(no_mangle)]
        #[doc(hidden)]
        pub extern "C" fn frame_entry() -> ! {
            ::aether_frame::retain();
            #fn_name()
        }
    };

    output.into()
}
