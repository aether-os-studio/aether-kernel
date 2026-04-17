extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn::{Ident, ItemFn, LitStr, ReturnType, Token, Type, parse::Parse, parse_macro_input};

struct FrameEntryArgs {
    secondary: LitStr,
}

impl Parse for FrameEntryArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let key: Ident = input.parse()?;
        if key != "secondary" {
            return Err(syn::Error::new(
                key.span(),
                "expected `secondary = \"...\"`",
            ));
        }
        input.parse::<Token![=]>()?;
        let secondary = input.parse::<LitStr>()?;
        Ok(Self { secondary })
    }
}

#[proc_macro_attribute]
pub fn frame_entry(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(_attr as FrameEntryArgs);
    let input_fn = parse_macro_input!(item as ItemFn);
    let fn_attrs = &input_fn.attrs;
    let fn_vis = &input_fn.vis;
    let fn_name = &input_fn.sig.ident;
    let fn_block = &input_fn.block;
    let secondary = match syn::parse_str::<Ident>(&args.secondary.value()) {
        Ok(value) => value,
        Err(error) => return error.into_compile_error().into(),
    };

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
        pub extern "C" fn kernel_frame_main() -> ! {
            ::aether_frame::retain();
            #fn_name()
        }

        #[unsafe(no_mangle)]
        #[doc(hidden)]
        pub extern "C" fn kernel_frame_secondary_main(cpu_index: usize) -> ! {
            ::aether_frame::retain();
            #secondary(cpu_index)
        }
    };

    output.into()
}
