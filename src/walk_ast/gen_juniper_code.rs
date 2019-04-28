mod directives;

use self::directives::panic_if_has_directives;
use super::{graphql_scalar_type_to_rust_type, ident, quote_ident, type_name, Output, TypeKind};
use crate::nullable_type::NullableType;
use graphql_parser::{
    query::{Name, Type},
    schema::*,
};
use heck::{CamelCase, SnakeCase};
use lazy_static::lazy_static;
use proc_macro2::TokenStream;
use quote::quote;
use regex::Regex;
use syn::Ident;

pub fn gen_juniper_code(doc: Document, error_type: syn::Type, out: &mut Output) {
    gen_enum_from_name(out);

    gen_doc(doc, &error_type, out);
}

fn gen_enum_from_name(out: &mut Output) {
    out.extend(quote! {
        /// Trait generated by juniper-from-schema
        ///
        /// Used for mapping GraphQL enums such as `METER` to a Rust enum such as `Unit::Meter`.
        ///
        /// You shouldn't have to interact with this type directly
        #[allow(dead_code)]
        trait EnumFromGraphQlName {
            #[allow(missing_docs)]
            fn from_name(name: &str) -> Self;
        }
    });
}

fn gen_doc(doc: Document, error_type: &syn::Type, out: &mut Output) {
    for def in doc.definitions {
        gen_def(def, error_type, out);
    }
}

fn gen_def(def: Definition, error_type: &syn::Type, out: &mut Output) {
    use graphql_parser::schema::Definition::*;

    match def {
        DirectiveDefinition(_) => not_supported!("Directives"),
        SchemaDefinition(schema_def) => gen_schema_def(schema_def, out),
        TypeDefinition(type_def) => gen_type_def(type_def, error_type, out),
        TypeExtension(_) => not_supported!("Extensions"),
    }
}

fn gen_schema_def(schema_def: SchemaDefinition, out: &mut Output) {
    if schema_def.subscription.is_some() {
        not_supported!("Subscriptions");
    }

    panic_if_has_directives(&schema_def);

    let query = match schema_def.query {
        Some(query) => ident(query),
        None => panic!("Juniper requires that the schema type has a query"),
    };

    let mutation = match schema_def.mutation {
        Some(mutation) => quote_ident(mutation),
        None => quote! { juniper::EmptyMutation<Context> },
    };

    out.extend(quote! {
        /// The GraphQL schema type generated by `juniper-from-schema`.
        pub type Schema = juniper::RootNode<'static, #query, #mutation>;
    })
}

fn gen_type_def(type_def: TypeDefinition, error_type: &syn::Type, out: &mut Output) {
    use graphql_parser::schema::TypeDefinition::*;

    match type_def {
        Enum(enum_type) => gen_enum_type(enum_type, out),
        Object(obj_type) => gen_obj_type(obj_type, error_type, out),
        Scalar(scalar_type) => gen_scalar_type(scalar_type, out),
        InputObject(input_type) => gen_input_def(input_type, out),
        Interface(interface_type) => gen_interface(interface_type, error_type, out),
        Union(union_type) => gen_union(&union_type, out),
    }
}

fn gen_input_def(input_type: InputObjectType, out: &mut Output) {
    panic_if_has_directives(&input_type);

    let name = ident(input_type.name);

    let fields = input_type.fields.into_iter().map(|field| {
        let arg = argument_to_name_and_rust_type(&field, &out);
        let name = ident(arg.name);
        let rust_type = arg.macro_type;

        let description = doc_tokens(&field.description);

        quote! {
            #[allow(missing_docs)]
            #description
            #name: #rust_type
        }
    });

    let description = doc_tokens(&input_type.description);

    out.extend(quote! {
        #[derive(juniper::GraphQLInputObject, Debug)]
        #description
        pub struct #name {
            #(#fields),*
        }
    })
}

fn gen_enum_type(enum_type: EnumType, out: &mut Output) {
    panic_if_has_directives(&enum_type);

    let name = to_enum_name(&enum_type.name);

    let trait_match_arms = enum_type
        .values
        .iter()
        .map(|value| {
            let graphql_name = &value.name;
            let variant = to_enum_name(&value.name);
            quote! {
                #graphql_name => #name::#variant,
            }
        })
        .collect::<Vec<_>>();

    let values = gen_with(gen_enum_value, enum_type.values, &out);

    let description = doc_tokens(&enum_type.description);

    out.extend(quote! {
        #description
        #[derive(juniper::GraphQLEnum, Debug, Eq, PartialEq, Copy, Clone, Hash)]
        pub enum #name {
            #values
        }
    });

    out.extend(quote! {
        impl EnumFromGraphQlName for #name {
            fn from_name(name: &str) -> Self {
                match name {
                    #(#trait_match_arms)*
                    _ => panic!("The variant {:?} for `{}` is unknown", name, stringify!(#name)),
                }
            }
        }
    })
}

fn to_enum_name(name: &str) -> Ident {
    ident(name.to_camel_case())
}

fn gen_enum_value(enum_value: EnumValue, out: &mut Output) {
    panic_if_has_directives(&enum_value);

    let graphql_name = enum_value.name;
    let name = to_enum_name(&graphql_name);
    let description = doc_tokens(&enum_value.description);

    out.extend(quote! {
        #[allow(missing_docs)]
        #[graphql(name=#graphql_name)]
        #description
        #name,
    })
}

fn gen_scalar_type(scalar_type: ScalarType, out: &mut Output) {
    panic_if_has_directives(&scalar_type);

    match &*scalar_type.name {
        "Date" => {}
        "DateTime" => {}
        name => {
            let name = ident(name);
            let description = scalar_type
                .description
                .map(|desc| quote! { description: #desc })
                .unwrap_or(quote! {});

            gen_scalar_type_with_data(&name, &description, out);
        }
    };
}

fn gen_scalar_type_with_data(name: &Ident, description: &TokenStream, out: &mut Output) {
    out.extend(quote! {
        /// Custom scalar type generated by `juniper-from-schema`.
        #[derive(Debug)]
        pub struct #name(pub String);

        juniper::graphql_scalar!(#name {
            #description

            resolve(&self) -> juniper::Value {
                juniper::Value::string(&self.0)
            }

            from_input_value(v: &InputValue) -> Option<#name> {
                v.as_string_value().map(|s| #name::new(s.to_owned()))
            }

            from_str<'a>(value: ScalarToken<'a>) -> juniper::ParseScalarResult<'a> {
                <String as juniper::ParseScalarValue>::from_str(value)
            }
        });

        impl #name {
            fn new<T: Into<String>>(t: T) -> Self {
                #name(t.into())
            }
        }
    })
}

fn trait_map_for_struct_name(struct_name: &Ident) -> Ident {
    ident(format!("{}Fields", struct_name))
}

fn gen_obj_type(obj_type: ObjectType, error_type: &syn::Type, out: &mut Output) {
    panic_if_has_directives(&obj_type);

    let struct_name = ident(obj_type.name);

    let trait_name = trait_map_for_struct_name(&struct_name);

    let field_tokens = obj_type
        .fields
        .into_iter()
        .map(|field| collect_data_for_field_gen(field, &out))
        .collect::<Vec<_>>();

    let trait_methods = field_tokens.iter().map(|field| {
        let field_name = &field.field_method;
        let field_type = &field.field_type;

        let args = &field.trait_args;

        match field.type_kind {
            TypeKind::Scalar => {
                quote! {
                    /// Field method generated by `juniper-from-schema`.
                    fn #field_name<'a>(
                        &self,
                        executor: &juniper::Executor<'a, Context>,
                        #(#args),*
                    ) -> std::result::Result<#field_type, #error_type>;
                }
            }
            TypeKind::Type => {
                let query_trail_type = ident(&field.inner_type);
                let trail = quote! { &QueryTrail<'a, #query_trail_type, Walked> };
                quote! {
                    /// Field method generated by `juniper-from-schema`.
                    fn #field_name<'a>(
                        &self,
                        executor: &juniper::Executor<'a, Context>,
                        trail: #trail, #(#args),*
                    ) -> std::result::Result<#field_type, #error_type>;
                }
            }
        }
    });

    out.extend(quote! {
        /// Trait for GraphQL field methods generated by `juniper-from-schema`.
        pub trait #trait_name {
            #(#trait_methods)*
        }
    });

    let fields = field_tokens
        .into_iter()
        .map(|field| gen_field(field, &struct_name, &trait_name, error_type));

    let description = obj_type
        .description
        .map(|d| quote! { description: #d })
        .unwrap_or_else(empty_token_stream);

    let interfaces = if obj_type.implements_interfaces.is_empty() {
        empty_token_stream()
    } else {
        let interface_names = obj_type.implements_interfaces.iter().map(|name| {
            let name = ident(name);
            quote! { &#name }
        });
        quote! { interfaces: [#(#interface_names),*] }
    };

    out.extend(quote! {
        juniper::graphql_object!(#struct_name: Context |&self| {
            #description
            #(#fields)*
            #interfaces
        });
    })
}

fn gen_field(
    field: FieldTokens,
    struct_name: &Ident,
    trait_name: &Ident,
    error_type: &syn::Type,
) -> TokenStream {
    let field_name = &field.name;
    let field_type = &field.field_type;
    let args = &field.macro_args;

    let body = gen_field_body(&field, &quote! { &self }, struct_name, trait_name);

    let description = field.description.unwrap_or_else(|| String::new());

    let all_args = to_field_args_list(args);

    quote! {
        #[doc = #description]
        field #field_name(#all_args) -> std::result::Result<#field_type, #error_type> {
            #body
        }
    }
}

fn gen_field_body(
    field: &FieldTokens,
    self_tokens: &TokenStream,
    struct_name: &Ident,
    trait_name: &Ident,
) -> TokenStream {
    let field_method = &field.field_method;
    let params = &field.params;

    match field.type_kind {
        TypeKind::Scalar => {
            quote! {
                <#struct_name as self::#trait_name>::#field_method(#self_tokens, &executor, #(#params),*)
            }
        }
        TypeKind::Type => {
            let query_trail_type = ident(&field.inner_type);
            quote! {
                let look_ahead = executor.look_ahead();
                let trail = look_ahead.make_query_trail::<#query_trail_type>();
                <#struct_name as self::#trait_name>::#field_method(#self_tokens, &executor, &trail, #(#params),*)
            }
        }
    }
}

fn gen_interface(interface: InterfaceType, error_type: &syn::Type, out: &mut Output) {
    panic_if_has_directives(&interface);

    let interface_name = ident(&interface.name);

    let description = interface
        .description
        .map(|d| d.to_string())
        .unwrap_or_else(String::new);

    let implementors = out.interface_implementors().get(&interface.name);

    let implementors = if let Some(implementors) = implementors {
        implementors
    } else {
        panic!("There are no implementors of {}", interface.name)
    };

    let implementors = implementors.iter().map(ident).collect::<Vec<_>>();

    // Enum
    let variants = implementors.iter().map(|name| {
        quote! { #name(#name) }
    });
    out.extend(quote! {
        pub enum #interface_name {
            #(#variants),*
        }
    });

    // From implementations
    for variant in &implementors {
        out.extend(quote! {
            impl std::convert::From<#variant> for #interface_name {
                fn from(x: #variant) -> #interface_name {
                    #interface_name::#variant(x)
                }
            }
        });
    }

    // Resolvers
    let instance_resolvers = implementors.iter().map(|name| {
        quote! {
            &#name => match *self { #interface_name::#name(ref h) => Some(h), _ => None }
        }
    });

    let field_tokens: Vec<FieldTokens> = interface
        .fields
        .into_iter()
        .map(|field| collect_data_for_field_gen(field, &out))
        .collect::<Vec<_>>();

    let field_token_streams = field_tokens
        .into_iter()
        .map(|field| {
            let field_name = &field.name;
            let args = &field.macro_args;
            let field_type = &field.field_type;

            let description = doc_tokens(&field.description);

            let arms = implementors.iter().map(|variant| {
                let trait_name = trait_map_for_struct_name(&variant);
                let struct_name = variant;

                let body = gen_field_body(&field, &quote! {inner}, &struct_name, &trait_name);

                quote! {
                    #interface_name::#struct_name(ref inner) => {
                        #body
                    }
                }
            });

            let all_args = to_field_args_list(&args);

            quote! {
                #description
                field #field_name(#all_args) -> std::result::Result<#field_type, #error_type> {
                    match *self {
                        #(#arms),*
                    }
                }
            }
        })
        .collect::<Vec<_>>();

    out.extend(quote! {
        graphql_interface!(#interface_name: Context |&self| {
            description: #description

            #(#field_token_streams)*

            instance_resolvers: |_| {
                #(#instance_resolvers),*
            }
        });
    });
}

fn to_field_args_list(args: &[TokenStream]) -> TokenStream {
    if args.is_empty() {
        quote! { &executor }
    } else {
        quote! { &executor, #(#args),* }
    }
}

fn gen_union(union: &UnionType, out: &mut Output) {
    panic_if_has_directives(union);

    let union_name = ident(&union.name);
    let implementors = union.types.iter().map(ident).collect::<Vec<_>>();

    // Enum
    let variants = implementors.iter().map(|name| {
        quote! { #name(#name) }
    });
    out.extend(quote! {
        pub enum #union_name {
            #(#variants),*
        }
    });

    // From implementations
    for variant in &implementors {
        out.extend(quote! {
            impl std::convert::From<#variant> for #union_name {
                fn from(x: #variant) -> #union_name {
                    #union_name::#variant(x)
                }
            }
        })
    }

    // Resolvers
    let instance_resolvers = implementors.iter().map(|name| {
        quote! {
            &#name => match *self { #union_name::#name(ref h) => Some(h), _ => None }
        }
    });

    let description = union
        .description
        .as_ref()
        .map(|d| d.to_string())
        .unwrap_or_else(String::new);

    out.extend(quote! {
        graphql_union!(#union_name: Context |&self| {
            description: #description

            instance_resolvers: |_| {
                #(#instance_resolvers),*
            }
        });
    });
}

fn empty_token_stream() -> TokenStream {
    quote! {}
}

#[derive(Debug, Clone)]
struct FieldTokens {
    name: Ident,
    macro_args: Vec<TokenStream>,
    trait_args: Vec<TokenStream>,
    field_type: TokenStream,
    field_method: Ident,
    params: Vec<TokenStream>,
    description: Option<String>,
    type_kind: TypeKind,
    inner_type: Name,
}

fn collect_data_for_field_gen(field: Field, out: &Output) -> FieldTokens {
    panic_if_has_directives(&field);

    let name = ident(field.name);

    let inner_type = type_name(&field.field_type).to_camel_case();

    let description = field.description.clone();

    let attributes = field
        .description
        .map(|d| parse_attributes(&d))
        .unwrap_or_else(Attributes::default);

    let (field_type, type_kind) = gen_field_type(
        &field.field_type,
        &FieldTypeDestination::Return(attributes),
        false,
        out,
    );

    let field_method = ident(format!("field_{}", name.to_string().to_snake_case()));

    let args_data = field
        .arguments
        .into_iter()
        .map(|input_value| argument_to_name_and_rust_type(&input_value, out))
        .collect::<Vec<_>>();

    let macro_args = args_data
        .iter()
        .map(|arg| {
            let name = ident(&arg.name);
            let arg_type = &arg.macro_type;
            let description = doc_tokens(&arg.description);
            quote! {
                #description
                #name: #arg_type
            }
        })
        .collect::<Vec<_>>();

    let trait_args = args_data
        .iter()
        .map(|arg| {
            let name = ident(&arg.name);
            let arg_type = &arg.trait_type;
            quote! { #name: #arg_type }
        })
        .collect::<Vec<_>>();

    let params = args_data
        .iter()
        .map(|arg| {
            let name = ident(&arg.name);
            if let Some(default_value) = &arg.default_value {
                quote! {
                    #name.unwrap_or_else(|| #default_value)
                }
            } else {
                quote! { #name }
            }
        })
        .collect::<Vec<_>>();

    FieldTokens {
        name,
        macro_args,
        trait_args,
        field_type,
        field_method,
        params,
        description,
        type_kind,
        inner_type,
    }
}

fn argument_to_name_and_rust_type(arg: &InputValue, out: &Output) -> FieldArgument {
    panic_if_has_directives(arg);

    let default_value = arg.default_value.as_ref().map(|value| quote_value(&value));

    let arg_name = arg.name.to_snake_case();

    let (macro_type, _) =
        gen_field_type(&arg.value_type, &FieldTypeDestination::Argument, false, out);

    let (trait_type, _) = gen_field_type(
        &arg.value_type,
        &FieldTypeDestination::Argument,
        default_value.is_some(),
        out,
    );

    FieldArgument {
        name: arg_name,
        macro_type,
        trait_type,
        default_value,
        description: arg.description.clone(),
    }
}

struct FieldArgument {
    name: Name,
    macro_type: TokenStream,
    trait_type: TokenStream,
    default_value: Option<TokenStream>,
    description: Option<String>,
}

fn quote_value(value: &Value) -> TokenStream {
    match value {
        Value::Float(inner) => quote! { #inner },
        Value::Int(inner) => {
            let number = inner
                .as_i64()
                .expect("failed to convert default number argument to i64");
            let number =
                i32_from_i64(number).expect("failed to convert default number argument to i64");
            quote! { #number }
        }
        Value::String(inner) => quote! { #inner.to_string() },
        Value::Boolean(inner) => quote! { #inner },

        Value::Enum(name) => {
            quote! { EnumFromGraphQlName::from_name(#name) }
        },

        Value::List(list) => {
            let mut acc = quote! { let mut vec = Vec::new(); };
            for value in list {
                let value_quoted = quote_value(value);
                acc.extend(quote! { vec.push(#value_quoted); });
            }
            acc.extend(quote! { vec });
            quote! { { #acc } }
        },

        // Object is hard because the contained BTreeMap can have values of different types.
        // How do we quote such a map and convert it into the actual input type?
        Value::Object(_map) => panic!("Default arguments where the type is an object is currently not supported."),

        Value::Variable(_name) => panic!("Default arguments cannot refer to variables."),
        Value::Null => panic!("Having a default argument value of `null` is not supported. Use a nullable type instead."),
    }
}

// This can also be with TryInto, but that requires 1.34
fn i32_from_i64(i: i64) -> Option<i32> {
    if i > std::i32::MAX as i64 {
        None
    } else {
        Some(i as i32)
    }
}

enum FieldTypeDestination {
    Argument,
    Return(Attributes),
}

fn gen_field_type(
    field_type: &Type,
    destination: &FieldTypeDestination,
    has_default_value: bool,
    out: &Output,
) -> (TokenStream, TypeKind) {
    let field_type = NullableType::from_schema_type(field_type.clone());

    if has_default_value && !field_type.is_nullable() {
        panic!("Fields with default arguments values must be nullable");
    }

    let field_type = if has_default_value {
        field_type.remove_one_layer_of_nullability()
    } else {
        field_type
    };

    let (tokens, ty) = gen_nullable_field_type(field_type, out);

    match (destination, ty) {
        (FieldTypeDestination::Return(attrs), ref ty) => match attrs.ownership() {
            Ownership::Owned => (tokens, *ty),
            Ownership::Borrowed => (quote! { &#tokens }, *ty),
        },

        (FieldTypeDestination::Argument, ty @ TypeKind::Scalar) => (tokens, ty),
        (FieldTypeDestination::Argument, ty @ TypeKind::Type) => (tokens, ty),
    }
}

fn gen_nullable_field_type(field_type: NullableType, out: &Output) -> (TokenStream, TypeKind) {
    use crate::nullable_type::NullableType::*;

    match field_type {
        NamedType(name) => graphql_scalar_type_to_rust_type(&name, &out),
        ListType(item_type) => {
            let (item_type, ty) = gen_nullable_field_type(*item_type, &out);
            (quote! { Vec<#item_type> }, ty)
        }
        NullableType(item_type) => {
            let (item_type, ty) = gen_nullable_field_type(*item_type, &out);
            (quote! { Option<#item_type> }, ty)
        }
    }
}

fn gen_with<F, T>(f: F, ts: Vec<T>, other: &Output) -> TokenStream
where
    F: Fn(T, &mut Output),
{
    let mut acc = other.clone_without_tokens();
    for t in ts {
        f(t, &mut acc);
    }
    acc.tokens().into_iter().collect::<TokenStream>()
}

#[derive(Debug, Eq, PartialEq)]
enum Attribute {
    Ownership(Ownership),
}

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
enum Ownership {
    Borrowed,
    Owned,
}

#[derive(Debug, Eq, PartialEq)]
struct Attributes {
    list: Vec<Attribute>,
}

impl std::default::Default for Attributes {
    fn default() -> Self {
        Attributes { list: Vec::new() }
    }
}

impl Attributes {
    #[allow(clippy::never_loop)]
    fn ownership(&self) -> Ownership {
        for attr in &self.list {
            match attr {
                Attribute::Ownership(x) => return *x,
            }
        }

        Ownership::Borrowed
    }
}

fn parse_attributes(desc: &str) -> Attributes {
    let attrs = desc
        .lines()
        .filter_map(|line| parse_attributes_line(line))
        .collect();
    Attributes { list: attrs }
}

lazy_static! {
    static ref ATTRIBUTE_PATTERN: Regex =
        Regex::new(r"\s*#\[(?P<key>\w+)\((?P<value>\w+)\)\]").unwrap();
}

fn parse_attributes_line(line: &str) -> Option<Attribute> {
    let caps = ATTRIBUTE_PATTERN.captures(line)?;
    let key = caps.name("key")?.as_str();
    let value = caps.name("value")?.as_str();

    let attr = match key {
        "ownership" => {
            let value = match value {
                "borrowed" => Ownership::Borrowed,
                "owned" => Ownership::Owned,
                _ => panic!("Unsupported attribute value '{}' for key '{}'", value, key),
            };
            Attribute::Ownership(value)
        }
        _ => panic!("Unsupported attribute key '{}'", key),
    };

    Some(attr)
}

fn doc_tokens(doc: &Option<String>) -> TokenStream {
    if let Some(doc) = doc {
        quote! {
            #[doc = #doc]
        }
    } else {
        quote! {}
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parse_descriptions_for_attributes() {
        let desc = r#"
        Comment

        #[ownership(borrowed)]
        "#;
        let attributes = parse_attributes(desc);
        assert_eq!(attributes.ownership(), Ownership::Borrowed);

        let desc = r#"
        Comment

        #[ownership(owned)]
        "#;
        let attributes = parse_attributes(desc);
        assert_eq!(attributes.ownership(), Ownership::Owned);

        let desc = r#"
        Comment
        "#;
        let attributes = parse_attributes(desc);
        assert_eq!(attributes.ownership(), Ownership::Borrowed);
    }
}
