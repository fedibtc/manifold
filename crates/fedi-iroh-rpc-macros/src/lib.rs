use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use quote::{format_ident, quote};
use syn::{
    FnArg, Ident, ItemTrait, Pat, PatIdent, ReturnType, TraitItem, TraitItemFn, Type,
    parse_macro_input, parse_quote,
};

/// Generate a client and a server for an RPC service trait.
///
/// `#[service(api_version = Type)]` versions the service's API: every method
/// then names the API version that added it in its current shape with
/// `#[added_in(expr)]`, where `expr` is a constant of `Type`, and the client
/// gets `METHOD_API_VERSIONS`, each method's wire name with that version.
#[proc_macro_attribute]
pub fn service(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut api_version = None;
    let attr_parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("api_version") {
            api_version = Some(meta.value()?.parse::<Type>()?);
            Ok(())
        } else {
            Err(meta.error("unsupported #[service] argument; expected `api_version = Type`"))
        }
    });
    parse_macro_input!(attr with attr_parser);
    let trait_item = parse_macro_input!(item as ItemTrait);
    match expand_service(trait_item, api_version) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Each method's `#[added_in(expr)]`, in method order. Every method of a
/// versioned service has exactly one, and an unversioned service has none.
fn added_in_versions(
    trait_item: &ItemTrait,
    api_version: Option<&Type>,
) -> syn::Result<Vec<syn::Expr>> {
    let mut error = None;
    let mut versions = Vec::new();
    for item in &trait_item.items {
        let TraitItem::Fn(method) = item else {
            continue;
        };
        let attrs = method
            .attrs
            .iter()
            .filter(|attr| attr.path().is_ident("added_in"))
            .collect::<Vec<_>>();
        match (api_version, attrs.as_slice()) {
            (Some(_), [attr]) => match attr.parse_args::<syn::Expr>() {
                Ok(version) => versions.push(version),
                Err(parse_error) => push_error(&mut error, parse_error),
            },
            (Some(_), []) => push_error(
                &mut error,
                syn::Error::new_spanned(
                    &method.sig.ident,
                    "methods of a #[service(api_version = ...)] trait need #[added_in(version)]",
                ),
            ),
            (Some(_), [_, extra, ..]) => push_error(
                &mut error,
                syn::Error::new_spanned(extra, "a method has one #[added_in(version)]"),
            ),
            (None, []) => {}
            (None, [attr, ..]) => push_error(
                &mut error,
                syn::Error::new_spanned(
                    attr,
                    "#[added_in] needs #[service(api_version = Type)] on the trait",
                ),
            ),
        }
    }
    match error {
        Some(error) => Err(error),
        None => Ok(versions),
    }
}

fn expand_service(
    trait_item: ItemTrait,
    api_version: Option<Type>,
) -> syn::Result<proc_macro2::TokenStream> {
    validate_trait(&trait_item)?;
    let added_in = added_in_versions(&trait_item, api_version.as_ref())?;
    let mut generated_trait = trait_item.clone();
    let trait_ident = &trait_item.ident;
    let client_ident = format_ident!("{}Client", trait_ident);
    let transport_client_ident = format_ident!("{}Transport", client_ident);
    let server_ident = format_ident!("{}Server", trait_ident);
    let rpc = rpc_crate_path();
    let methods = trait_item
        .items
        .iter()
        .filter_map(|item| match item {
            TraitItem::Fn(method) => Some(parse_method(method)),
            _ => None,
        })
        .collect::<Vec<_>>();

    for item in &mut generated_trait.items {
        if let TraitItem::Fn(method) = item {
            method
                .attrs
                .retain(|attr| !attr.path().is_ident("added_in"));
            let return_ty = match &method.sig.output {
                ReturnType::Type(_, ty) => *ty.clone(),
                ReturnType::Default => parse_quote!(()),
            };
            method.sig.asyncness = None;
            method.sig.output =
                parse_quote!(-> impl ::std::future::Future<Output = #return_ty> + Send);
        }
    }
    let client_methods = methods.iter().map(|method| {
        let ident = &method.ident;
        let req_ident = &method.request_ident;
        let req_ty = &method.request_ty;
        let ret_ty = &method.return_ty;
        let method_name = ident.to_string();
        quote! {
            async fn #ident(&self, #req_ident: #req_ty) -> #ret_ty {
                match self.rpc.call::<_, #ret_ty>(#method_name, #req_ident).await {
                    Ok(response) => response,
                    Err(error) => <#ret_ty as #rpc::RpcReturn>::from_rpc_error(error),
                }
            }
        }
    });
    let raw_client_methods = methods.iter().map(|method| {
        let ident = &method.ident;
        let req_ident = &method.request_ident;
        let req_ty = &method.request_ty;
        let ret_ty = &method.return_ty;
        let method_name = ident.to_string();
        quote! {
            /// Call the RPC method while retaining transport failures in an
            /// outer result distinct from the serialized service return.
            pub async fn #ident(
                &self,
                #req_ident: #req_ty,
            ) -> #rpc::RpcResult<#ret_ty> {
                self.rpc.call(#method_name, #req_ident).await
            }
        }
    });

    let server_arms = methods.iter().map(|method| {
        let ident = &method.ident;
        let req_ty = &method.request_ty;
        let ret_ty = &method.return_ty;
        let method_name = ident.to_string();
        quote! {
            #method_name => {
                #rpc::__private::decode_call::<#req_ty, #ret_ty, _>(
                    body,
                    |request| self.service.#ident(request),
                ).await
            }
        }
    });

    let method_api_versions = api_version.map(|api_version| {
        let entries = methods.iter().zip(&added_in).map(|(method, version)| {
            let method_name = method.ident.to_string();
            quote! { (#method_name, #version) }
        });
        quote! {
            impl #client_ident {
                /// Each method's wire name and the API version that added it
                /// in its current shape, in declaration order.
                pub const METHOD_API_VERSIONS: &'static [(&'static str, #api_version)] =
                    &[#(#entries),*];
            }
        }
    });

    Ok(quote! {
        #generated_trait

        #method_api_versions

        #[derive(Debug, Clone)]
        pub struct #client_ident {
            rpc: #rpc::RpcClient,
        }

        impl #client_ident {
            #[must_use]
            pub fn new(connection: #rpc::iroh::endpoint::Connection) -> Self {
                Self {
                    rpc: #rpc::RpcClient::new(connection),
                }
            }

            #[must_use]
            pub fn from_rpc_client(rpc: #rpc::RpcClient) -> Self {
                Self { rpc }
            }

            /// Borrow a typed client that retains local RPC transport errors
            /// outside each serialized service return value.
            #[must_use]
            pub fn transport(&self) -> #transport_client_ident<'_> {
                #transport_client_ident { rpc: &self.rpc }
            }
        }

        /// Typed transport-preserving view of the generated RPC client.
        #[derive(Debug, Clone, Copy)]
        pub struct #transport_client_ident<'a> {
            rpc: &'a #rpc::RpcClient,
        }

        impl #transport_client_ident<'_> {
            #(#raw_client_methods)*
        }

        impl #trait_ident for #client_ident {
            #(#client_methods)*
        }

        #[derive(Debug, Clone)]
        pub struct #server_ident<S> {
            service: S,
        }

        impl<S> #server_ident<S> {
            #[must_use]
            pub fn new(service: S) -> Self {
                Self { service }
            }
        }

        #[#rpc::async_trait::async_trait]
        impl<S> #rpc::RpcServiceHandler for #server_ident<S>
        where
            S: #trait_ident + Send + Sync + 'static,
        {
            async fn handle_rpc(&self, method: &str, body: &[u8]) -> #rpc::RpcResult<Vec<u8>> {
                match method {
                    #(#server_arms,)*
                    _ => Err(#rpc::__private::unknown_method(method)),
                }
            }
        }
    })
}

fn rpc_crate_path() -> proc_macro2::TokenStream {
    match crate_name("fedi-iroh-rpc") {
        Ok(FoundCrate::Name(name)) => {
            let ident = Ident::new(&name, proc_macro2::Span::call_site());
            quote!(::#ident)
        }
        Ok(FoundCrate::Itself) | Err(_) => quote!(::fedi_iroh_rpc),
    }
}

fn validate_trait(trait_item: &ItemTrait) -> syn::Result<()> {
    let mut error = None;

    if !trait_item.generics.params.is_empty() || trait_item.generics.where_clause.is_some() {
        push_error(
            &mut error,
            syn::Error::new_spanned(
                &trait_item.generics,
                "#[service] does not support generic service traits",
            ),
        );
    }

    for item in &trait_item.items {
        match item {
            TraitItem::Fn(method) => validate_method(&mut error, method),
            _ => push_error(
                &mut error,
                syn::Error::new_spanned(
                    item,
                    "#[service] only supports async methods; associated items are not supported",
                ),
            ),
        }
    }

    match error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn validate_method(error: &mut Option<syn::Error>, method: &TraitItemFn) {
    if method.default.is_some() {
        push_error(
            error,
            syn::Error::new_spanned(method, "#[service] does not support default method bodies"),
        );
    }
    if method.sig.asyncness.is_none() {
        push_error(
            error,
            syn::Error::new_spanned(&method.sig.ident, "#[service] methods must be async"),
        );
    }
    if !method.sig.generics.params.is_empty() || method.sig.generics.where_clause.is_some() {
        push_error(
            error,
            syn::Error::new_spanned(
                &method.sig.generics,
                "#[service] does not support generic methods",
            ),
        );
    }
    if matches!(method.sig.output, ReturnType::Default) {
        push_error(
            error,
            syn::Error::new_spanned(
                &method.sig.ident,
                "#[service] methods must return a value, normally Result<Response, Error>",
            ),
        );
    }

    let mut inputs = method.sig.inputs.iter();
    match inputs.next() {
        Some(FnArg::Receiver(receiver))
            if receiver.reference.is_some() && receiver.mutability.is_none() => {}
        Some(arg) => push_error(
            error,
            syn::Error::new_spanned(
                arg,
                "#[service] methods must take &self as their first argument",
            ),
        ),
        None => push_error(
            error,
            syn::Error::new_spanned(&method.sig.ident, "#[service] methods must take &self"),
        ),
    }

    match inputs.next() {
        Some(FnArg::Typed(request)) => {
            if matches!(request.ty.as_ref(), Type::Reference(_)) {
                push_error(
                    error,
                    syn::Error::new_spanned(
                        &request.ty,
                        "#[service] request argument must be owned, not a reference",
                    ),
                );
            }
            if !matches!(request.pat.as_ref(), Pat::Ident(_)) {
                push_error(
                    error,
                    syn::Error::new_spanned(
                        &request.pat,
                        "#[service] request argument must be a plain identifier",
                    ),
                );
            }
        }
        Some(arg) => push_error(
            error,
            syn::Error::new_spanned(
                arg,
                "#[service] request argument must be an owned typed argument",
            ),
        ),
        None => push_error(
            error,
            syn::Error::new_spanned(
                &method.sig.ident,
                "#[service] methods must take exactly one request argument",
            ),
        ),
    }
    if let Some(arg) = inputs.next() {
        push_error(
            error,
            syn::Error::new_spanned(
                arg,
                "#[service] methods must take exactly one request argument",
            ),
        );
    }
}

fn push_error(target: &mut Option<syn::Error>, error: syn::Error) {
    if let Some(target) = target {
        target.combine(error);
    } else {
        *target = Some(error);
    }
}
struct Method {
    ident: Ident,
    request_ident: Ident,
    request_ty: Type,
    return_ty: Type,
}

fn parse_method(method: &TraitItemFn) -> Method {
    if method.sig.asyncness.is_none() {
        panic!("#[service] only supports async trait methods");
    }

    let mut inputs = method.sig.inputs.iter();
    match inputs.next() {
        Some(FnArg::Receiver(_)) => {}
        _ => panic!("#[service] methods must take &self as their first argument"),
    }

    let request = match inputs.next() {
        Some(FnArg::Typed(request)) => request,
        _ => panic!("#[service] methods must take exactly one request argument"),
    };
    if inputs.next().is_some() {
        panic!("#[service] methods must take exactly one request argument");
    }

    let request_ident = match request.pat.as_ref() {
        Pat::Ident(PatIdent { ident, .. }) => ident.clone(),
        _ => panic!("#[service] request argument must be a plain identifier"),
    };
    let request_ty = (*request.ty).clone();
    let return_ty = match &method.sig.output {
        ReturnType::Type(_, ty) => *ty.clone(),
        ReturnType::Default => syn::parse_quote!(()),
    };

    Method {
        ident: method.sig.ident.clone(),
        request_ident,
        request_ty,
        return_ty,
    }
}
