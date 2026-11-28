use proc_macro::TokenStream;

/// Route the attribute used by teloxide to the maintained documentation renderer.
#[proc_macro_attribute]
pub fn aquamarine(_args: TokenStream, item: TokenStream) -> TokenStream {
    let mut output: TokenStream = r#"#[::aquamarine::__merman(crate_path = "::aquamarine")]"#
        .parse()
        .expect("valid Merman attribute");
    output.extend(item);
    output
}
