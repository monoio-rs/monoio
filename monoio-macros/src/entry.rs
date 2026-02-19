// Heavily borrowed from tokio.
// Copyright (c) 2021 Tokio Contributors, licensed under the MIT license.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{quote, quote_spanned, ToTokens};
use syn::parse::Parser;

// syn::AttributeArgs does not implement syn::Parse
type AttributeArgs = syn::punctuated::Punctuated<syn::Meta, syn::Token![,]>;

#[derive(Clone, Copy)]
struct FinalConfig {
    entries: Option<u32>,
    timer_enabled: Option<bool>,
    threads: Option<u32>,
    driver: DriverType,
}

/// Config used in case of the attribute not being able to build a valid config
const DEFAULT_ERROR_CONFIG: FinalConfig = FinalConfig {
    entries: None,
    timer_enabled: None,
    threads: None,
    driver: DriverType::Fusion,
};

struct Configuration {
    entries: Option<(u32, Span)>,
    timer_enabled: Option<(bool, Span)>,
    threads: Option<(u32, Span)>,
    driver: Option<(DriverType, Span)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DriverType {
    Legacy,
    Uring,
    Iocp,
    Fusion,
}

impl Configuration {
    fn new() -> Self {
        Configuration {
            entries: None,
            timer_enabled: None,
            threads: None,
            driver: None,
        }
    }

    fn set_driver(&mut self, driver: syn::Lit, span: Span) -> Result<(), syn::Error> {
        if self.driver.is_some() {
            return Err(syn::Error::new(span, "`driver` set multiple times."));
        }

        let driver = parse_driver(driver, span, "driver")?;
        self.driver = Some((driver, span));
        Ok(())
    }

    fn set_threads(&mut self, threads: syn::Lit, span: Span) -> Result<(), syn::Error> {
        if self.threads.is_some() {
            return Err(syn::Error::new(span, "`threads` set multiple times."));
        }

        let threads = parse_int(threads, span, "threads")? as u32;
        self.threads = Some((threads, span));
        Ok(())
    }

    fn set_entries(&mut self, entries: syn::Lit, span: Span) -> Result<(), syn::Error> {
        if self.entries.is_some() {
            return Err(syn::Error::new(span, "`entries` set multiple times."));
        }

        let entries = parse_int(entries, span, "entries")? as u32;
        if entries == 0 {
            return Err(syn::Error::new(span, "`entries` may not be 0."));
        }
        self.entries = Some((entries, span));
        Ok(())
    }

    fn set_timer_enabled(&mut self, enabled: syn::Lit, span: Span) -> Result<(), syn::Error> {
        if self.timer_enabled.is_some() {
            return Err(syn::Error::new(span, "`timer_enabled` set multiple times."));
        }

        let enabled = parse_bool(enabled, span, "timer_enabled")?;
        self.timer_enabled = Some((enabled, span));
        Ok(())
    }

    fn build(&self) -> Result<FinalConfig, syn::Error> {
        Ok(FinalConfig {
            entries: self.entries.map(|(e, _)| e),
            timer_enabled: self.timer_enabled.map(|(t, _)| t),
            threads: self.threads.map(|(t, _)| t),
            driver: self.driver.map(|(d, _)| d).unwrap_or(DriverType::Fusion),
        })
    }
}

#[allow(unused)]
fn parse_int(int: syn::Lit, span: Span, field: &str) -> Result<usize, syn::Error> {
    match int {
        syn::Lit::Int(lit) => match lit.base10_parse::<usize>() {
            Ok(value) => Ok(value),
            Err(e) => Err(syn::Error::new(
                span,
                format!("Failed to parse value of `{field}` as integer: {e}"),
            )),
        },
        _ => Err(syn::Error::new(
            span,
            format!("Failed to parse value of `{field}` as integer."),
        )),
    }
}

#[allow(unused)]
fn parse_string(lit: syn::Lit, span: Span, field: &str) -> Result<String, syn::Error> {
    match lit {
        syn::Lit::Str(s) => Ok(s.value()),
        syn::Lit::Verbatim(s) => Ok(s.to_string()),
        _ => Err(syn::Error::new(
            span,
            format!("Failed to parse value of `{field}` as string."),
        )),
    }
}

#[allow(unused)]
fn parse_driver(lit: syn::Lit, span: Span, field: &str) -> Result<DriverType, syn::Error> {
    let val = parse_string(lit, span, field)?;
    match val.as_str() {
        "legacy" => Ok(DriverType::Legacy),
        "uring" | "io_uring" | "iouring" => Ok(DriverType::Uring),
        "cp" | "iocp" => Ok(DriverType::Iocp),
        "fusion" | "auto" | "detect" => Ok(DriverType::Fusion),
        _ => Err(syn::Error::new(
            span,
            format!("Failed to parse value of `{field}` as DriverType."),
        )),
    }
}

#[allow(unused)]
fn parse_bool(bool: syn::Lit, span: Span, field: &str) -> Result<bool, syn::Error> {
    match bool {
        syn::Lit::Bool(b) => Ok(b.value),
        _ => Err(syn::Error::new(
            span,
            format!("Failed to parse value of `{field}` as bool."),
        )),
    }
}

fn build_config(input: syn::ItemFn, args: AttributeArgs) -> Result<FinalConfig, syn::Error> {
    if input.sig.asyncness.is_none() {
        let msg = "the `async` keyword is missing from the function declaration";
        return Err(syn::Error::new_spanned(input.sig.fn_token, msg));
    }

    let mut config = Configuration::new();

    for arg in args {
        match arg {
            syn::Meta::NameValue(namevalue) => {
                let ident = namevalue
                    .path
                    .get_ident()
                    .ok_or_else(|| {
                        syn::Error::new_spanned(&namevalue, "Must have specified ident")
                    })?
                    .to_string()
                    .to_lowercase();
                let lit = match &namevalue.value {
                    syn::Expr::Lit(syn::ExprLit { lit, .. }) => lit,
                    expr => return Err(syn::Error::new_spanned(expr, "Must be a literal")),
                };
                match ident.as_str() {
                    "entries" => {
                        config.set_entries(lit.clone(), syn::spanned::Spanned::span(lit))?
                    }
                    "timer_enabled" | "enable_timer" | "timer" => {
                        config.set_timer_enabled(lit.clone(), syn::spanned::Spanned::span(lit))?
                    }
                    "worker_threads" | "workers" | "threads" => {
                        config.set_threads(lit.clone(), syn::spanned::Spanned::span(lit))?;
                        // Function must return `()` since it will be swallowed.
                        if !matches!(config.threads, None | Some((1, _)))
                            && !matches!(input.sig.output, syn::ReturnType::Default)
                        {
                            return Err(syn::Error::new(
                                syn::spanned::Spanned::span(lit),
                                "With multi-thread function can not have a return value",
                            ));
                        }
                    }
                    "driver" => config.set_driver(lit.clone(), syn::spanned::Spanned::span(lit))?,
                    name => {
                        let msg = format!(
                            "Unknown attribute {name} is specified; expected one of: \
                             `worker_threads`, `entries`, `timer_enabled`",
                        );
                        return Err(syn::Error::new_spanned(namevalue, msg));
                    }
                }
            }
            syn::Meta::Path(path) => {
                let name = path
                    .get_ident()
                    .ok_or_else(|| syn::Error::new_spanned(&path, "Must have specified ident"))?
                    .to_string()
                    .to_lowercase();
                let msg = format!(
                    "Unknown attribute {name} is specified; expected one of: `worker_threads`, \
                     `entries`, `timer_enabled`"
                );
                return Err(syn::Error::new_spanned(path, msg));
            }
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "Unknown attribute inside the macro",
                ));
            }
        }
    }

    config.build()
}

fn parse_knobs(mut input: syn::ItemFn, is_test: bool, config: FinalConfig) -> TokenStream {
    input.sig.asyncness = None;

    // If type mismatch occurs, the current rustc points to the last statement.
    let (last_stmt_start_span, last_stmt_end_span) = {
        let mut last_stmt = input
            .block
            .stmts
            .last()
            .map(ToTokens::into_token_stream)
            .unwrap_or_default()
            .into_iter();
        // `Span` on stable Rust has a limitation that only points to the first
        // token, not the whole tokens. We can work around this limitation by
        // using the first/last span of the tokens like
        // `syn::Error::new_spanned` does.
        let start = last_stmt.next().map_or_else(Span::call_site, |t| t.span());
        let end = last_stmt.last().map_or(start, |t| t.span());
        (start, end)
    };

    let mut rt = match config.driver {
        DriverType::Legacy => {
            quote_spanned! {last_stmt_start_span=>monoio::RuntimeBuilder::<monoio::LegacyDriver>::new()}
        }
        DriverType::Uring => {
            quote_spanned! {last_stmt_start_span=>monoio::RuntimeBuilder::<monoio::IoUringDriver>::new()}
        }
        DriverType::Iocp => {
            quote_spanned! {last_stmt_start_span=>monoio::RuntimeBuilder::<monoio::IocpDriver>::new()}
        }
        DriverType::Fusion => {
            quote_spanned! {last_stmt_start_span=>monoio::RuntimeBuilder::<monoio::FusionDriver>::new()}
        }
    };

    if let Some(entries) = config.entries {
        rt = quote! { #rt.with_entries(#entries) }
    }
    if Some(true) == config.timer_enabled {
        rt = quote! { #rt.enable_timer() }
    }

    let body = &input.block;
    let brace_token = input.block.brace_token;
    let (tail_return, tail_semicolon) = match body.stmts.last() {
        Some(syn::Stmt::Expr(expr, Some(_))) => (
            match expr {
                syn::Expr::Return(_) => quote! { return },
                _ => quote! {},
            },
            quote! {
                ;
            },
        ),
        _ => (quote! {}, quote! {}),
    };

    if matches!(config.threads, None | Some(1)) {
        input.block = syn::parse2(quote_spanned! {last_stmt_end_span=>
            {
                let body = async #body;
                #[allow(clippy::expect_used)]
                #tail_return #rt
                    .build()
                    .expect("Failed building the Runtime")
                    .block_on(body)#tail_semicolon
            }
        })
        .expect("Parsing failure");
    } else {
        // Check covered when building config.
        debug_assert!(matches!(input.sig.output, syn::ReturnType::Default));

        let threads = config.threads.unwrap();
        let threads_expr = if threads == 0 {
            // auto detected parallism
            quote!(::std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1))
        } else {
            quote!(#threads)
        };
        input.block = syn::parse2(quote_spanned! {last_stmt_end_span=>
            {
                let body = async #body;

                #[allow(clippy::needless_collect)]
                let threads: Vec<_> = (1 .. #threads_expr)
                    .map(|_| {
                        ::std::thread::spawn(|| {
                            #rt.build()
                                .expect("Failed building the Runtime")
                                .block_on(async #body);
                        })
                    })
                    .collect();
                // Run on main threads
                #rt.build()
                    .expect("Failed building the Runtime")
                    .block_on(body);

                // Wait for other threads
                threads.into_iter().for_each(|t| {
                    let _ = t.join();
                });
            }
        })
        .expect("Parsing failure");
    }

    input.block.brace_token = brace_token;

    let header = if is_test {
        quote! {
            #[::core::prelude::v1::test]
        }
    } else {
        quote! {}
    };
    let cfg_attr = if is_test {
        match config.driver {
            DriverType::Legacy => quote! {
                #[cfg(feature = "legacy")]
            },
            DriverType::Uring => quote! {
                #[cfg(all(target_os = "linux", feature = "iouring"))]
            },
            DriverType::Iocp => quote! {
                #[cfg(all(windows, feature = "iocp"))]
            },
            DriverType::Fusion => quote! {
                #[cfg(any(feature = "legacy", feature = "iouring"))]
            },
        }
    } else {
        quote! {}
    };
    let result = quote! {
        #header
        #cfg_attr
        #input
    };
    result.into()
}

fn token_stream_with_error(mut tokens: TokenStream, error: syn::Error) -> TokenStream {
    tokens.extend(TokenStream::from(error.into_compile_error()));
    tokens
}

pub(crate) fn main(args: TokenStream, item: TokenStream) -> TokenStream {
    // If any of the steps for this macro fail, we still want to expand to an item that is as close
    // to the expected output as possible. This helps out IDEs such that completions and other
    // related features keep working.
    let input: syn::ItemFn = match syn::parse(item.clone()) {
        Ok(it) => it,
        Err(e) => return token_stream_with_error(item, e),
    };

    let config = if input.sig.ident == "main" && !input.sig.inputs.is_empty() {
        let msg = "the main function cannot accept arguments";
        Err(syn::Error::new_spanned(&input.sig.ident, msg))
    } else {
        AttributeArgs::parse_terminated
            .parse(args)
            .and_then(|args| build_config(input.clone(), args))
    };

    match config {
        Ok(config) => parse_knobs(input, false, config),
        Err(e) => token_stream_with_error(parse_knobs(input, false, DEFAULT_ERROR_CONFIG), e),
    }
}

pub(crate) fn test(args: TokenStream, item: TokenStream) -> TokenStream {
    // If any of the steps for this macro fail, we still want to expand to an item that is as close
    // to the expected output as possible. This helps out IDEs such that completions and other
    // related features keep working.
    let input: syn::ItemFn = match syn::parse(item.clone()) {
        Ok(it) => it,
        Err(e) => return token_stream_with_error(item, e),
    };
    let config = if let Some(attr) = input
        .attrs
        .iter()
        .find(|attr| attr.meta.path().is_ident("test"))
    {
        let msg = "second test attribute is supplied";
        Err(syn::Error::new_spanned(attr, msg))
    } else {
        AttributeArgs::parse_terminated
            .parse(args)
            .and_then(|args| build_config(input.clone(), args))
    };

    match config {
        Ok(config) => parse_knobs(input, true, config),
        Err(e) => token_stream_with_error(parse_knobs(input, true, DEFAULT_ERROR_CONFIG), e),
    }
}

pub(crate) fn test_all(args: TokenStream, item: TokenStream) -> TokenStream {
    // If any of the steps for this macro fail, we still want to expand to an item that is as close
    // to the expected output as possible. This helps out IDEs such that completions and other
    // related features keep working.
    let input: syn::ItemFn = match syn::parse(item.clone()) {
        Ok(it) => it,
        Err(e) => return token_stream_with_error(item, e),
    };
    let config = if let Some(attr) = input
        .attrs
        .iter()
        .find(|attr| attr.meta.path().is_ident("test"))
    {
        let msg = "second test attribute is supplied";
        Err(syn::Error::new_spanned(attr, msg))
    } else {
        AttributeArgs::parse_terminated
            .parse(args)
            .and_then(|args| build_config(input.clone(), args))
    };
    let mut config = match config {
        Ok(config) => config,
        Err(e) => {
            return token_stream_with_error(parse_knobs(input, true, DEFAULT_ERROR_CONFIG), e)
        }
    };

    let mut output = TokenStream::new();

    let mut input_uring = input.clone();
    input_uring.sig.ident = proc_macro2::Ident::new(
        &format!("uring_{}", input_uring.sig.ident),
        input_uring.sig.ident.span(),
    );
    config.driver = DriverType::Uring;
    let token_uring = parse_knobs(input_uring, true, config);
    output.extend(token_uring);

    let mut input_legacy = input.clone();
    input_legacy.sig.ident = proc_macro2::Ident::new(
        &format!("legacy_{}", input_legacy.sig.ident),
        input_legacy.sig.ident.span(),
    );
    config.driver = DriverType::Iocp;
    let token_legacy = parse_knobs(input_legacy, true, config);
    output.extend(token_legacy);

    let mut input_legacy = input;
    input_legacy.sig.ident = proc_macro2::Ident::new(
        &format!("legacy_{}", input_legacy.sig.ident),
        input_legacy.sig.ident.span(),
    );
    config.driver = DriverType::Legacy;
    let token_legacy = parse_knobs(input_legacy, true, config);
    output.extend(token_legacy);

    output
}
