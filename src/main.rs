#![cfg_attr(feature = "cargo-clippy", deny(clippy, clippy_pedantic))]
#![cfg_attr(feature = "cargo-clippy", allow(
	cyclomatic_complexity,
	default_trait_access,
	similar_names,
	too_many_arguments,
	type_complexity,
	unseparated_literal_suffix,
))]

extern crate backtrace;
extern crate env_logger;
#[macro_use]
extern crate log;
extern crate reqwest;
extern crate serde;
#[macro_use]
extern crate serde_derive;

mod fixups;
mod supported_version;
mod swagger20;

struct Error(Box<std::error::Error>, backtrace::Backtrace);

impl<E> From<E> for Error where E: Into<Box<std::error::Error>> {
	fn from(value: E) -> Self {
		Error(value.into(), backtrace::Backtrace::new())
	}
}

impl std::fmt::Debug for Error {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		writeln!(f, "{}", self.0)?;
		write!(f, "{:?}", self.1)?;
		Ok(())
	}
}

fn main() -> Result<(), Error> {
	{
		let mut builder = env_logger::Builder::new();
		builder.format(|buf, record| {
			use std::io::Write;
			writeln!(buf, "{} {}:{} {}", record.level(), record.file().unwrap_or("?"), record.line().unwrap_or(0), record.args())
		});
		let rust_log = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
		builder.parse(&rust_log);
		builder.init();
	}

	let client = reqwest::Client::new();

	let out_dir_base: &std::path::Path = env!("CARGO_MANIFEST_DIR").as_ref();
	let out_dir_base = out_dir_base.join("k8s-openapi").join("src");

	for &supported_version in supported_version::ALL {
		run(supported_version, &out_dir_base, &client)?;
	}

	Ok(())
}

fn run(supported_version: supported_version::SupportedVersion, out_dir_base: &std::path::Path, client: &reqwest::Client) -> Result<(), Error> {
	use std::io::Write;

	let mod_root = supported_version.mod_root();

	let out_dir = out_dir_base.join(mod_root);

	let replace_namespaces: &[(&[std::borrow::Cow<'static, str>], &[std::borrow::Cow<'static, str>])] = &[
		// Everything's under io.k8s, so strip it
		(&["io".into(), "k8s".into()], &[]),
	];

	let mut num_generated_structs = 0usize;
	let mut num_generated_type_aliases = 0usize;
	let mut num_generated_apis = 0usize;

	let mut spec: swagger20::Spec = {
		let spec_url = supported_version.spec_url();
		info!(target: "", "Parsing spec file at {} ...", spec_url);
		let mut response = client.get(spec_url).send()?;
		let status = response.status();
		if status != reqwest::StatusCode::OK {
			return Err(status.to_string().into());
		}
		response.json()?
	};

	supported_version.fixup(&mut spec)?;

	let expected_num_generated_types: usize = spec.definitions.len();
	let expected_num_generated_apis: usize = spec.paths.iter().map(|(_, path_item)| path_item.operations.len()).sum();

	info!(
		"OK. Spec has {} definitions and {} paths containing {} operations",
		spec.definitions.len(),
		spec.paths.len(),
		expected_num_generated_apis);

	let mut operations: std::collections::BTreeMap<_, Vec<_>> = Default::default();
	for (path, path_item) in &spec.paths {
		for operation in &path_item.operations {
			operations
			.entry(operation.kubernetes_group_kind_version.as_ref())
			.or_insert_with(Default::default)
			.push((path, path_item, operation));
		}
	}
	for operations in operations.values_mut() {
		operations.sort_by_key(|(_, _, operation)| &operation.id);
	}

	loop {
		info!("Removing output directory {} ...", out_dir.display());
		match std::fs::remove_dir_all(&out_dir) {
			Ok(()) => trace!("OK"),
			Err(ref err) if err.kind() == std::io::ErrorKind::NotFound => {
				trace!("OK. Directory doesn't exist");

				info!("Creating output directory {} ...", out_dir.display());
				match std::fs::create_dir(&out_dir) {
					Ok(()) => {
						trace!("OK");
						break;
					},
					Err(err) => error!("Error: {}", err),
				}
			},
			Err(err) => error!("Error: {}", err),
		}
	}

	info!("Generating types...");

	for (definition_path, definition) in &spec.definitions {
		trace!("Working on {} ...", definition_path);

		let (mut file, type_name, type_ref_path) = create_file_for_type(&definition_path, &out_dir, &replace_namespaces)?;

		writeln!(file, "// Generated from definition {}", definition_path)?;
		writeln!(file)?;

		if let Some(description) = &definition.description {
			for line in get_comment_text(description, "") {
				writeln!(file, "///{}", line)?;
			}
		}

		let can_be_default = can_be_default(&definition.kind, &spec)?;

		match &definition.kind {
			swagger20::SchemaKind::Properties(properties) => {
				struct Property<'a> {
					name: &'a swagger20::PropertyName,
					schema: &'a swagger20::Schema,
					required: bool,
					field_name: std::borrow::Cow<'static, str>,
					field_type_name: String,
				}

				let properties = {
					use std::fmt::Write;

					let mut result = Vec::with_capacity(properties.len());

					for (name, (schema, required)) in properties {
						let field_name = get_rust_ident(&name);

						let mut field_type_name = String::new();

						if !required {
							write!(field_type_name, "Option<")?;
						}

						let type_name = get_rust_type(&schema.kind, &replace_namespaces, mod_root)?;

						// Fix cases of infinite recursion
						if let swagger20::SchemaKind::Ref(ref ref_path) = schema.kind {
							match (&**definition_path, &**name, &**ref_path) {
								(
									"io.k8s.apiextensions-apiserver.pkg.apis.apiextensions.v1beta1.JSONSchemaProps",
									"not",
									"io.k8s.apiextensions-apiserver.pkg.apis.apiextensions.v1beta1.JSONSchemaProps",
								) => write!(field_type_name, "Box<{}>", type_name)?,

								_ => write!(field_type_name, "{}", type_name)?,
							}
						}
						else {
							write!(field_type_name, "{}", type_name)?;
						};

						if !required {
							write!(field_type_name, ">")?;
						}

						result.push(Property {
							name,
							schema,
							required: *required,
							field_name,
							field_type_name,
						});
					}

					result
				};

				write!(file, "#[derive(Clone, Debug")?;

				if can_be_default {
					write!(file, ", Default")?;
				}

				writeln!(file, ", PartialEq)]")?;

				writeln!(file, "pub struct {} {{", type_name)?;

				for (i, Property { schema, field_name, field_type_name, .. }) in properties.iter().enumerate() {
					if i > 0 {
						writeln!(file)?;
					}

					if let Some(ref description) = schema.description {
						for line in get_comment_text(description, "") {
							writeln!(file, "    ///{}", line)?;
						}
					}

					write!(file, "    pub {}: ", field_name)?;

					write!(file, "{}", field_type_name)?;

					writeln!(file, ",")?;
				}
				writeln!(file, "}}")?;

				if let Some(kubernetes_group_kind_versions) = &definition.kubernetes_group_kind_versions {
					let mut kubernetes_group_kind_versions: Vec<_> = kubernetes_group_kind_versions.into_iter().collect();
					kubernetes_group_kind_versions.sort();
					for kubernetes_group_kind_version in kubernetes_group_kind_versions {
						if let Some(operations) = operations.remove(&Some(kubernetes_group_kind_version)) {
							writeln!(file)?;
							writeln!(file, "// Begin {}/{}/{}",
								kubernetes_group_kind_version.group, kubernetes_group_kind_version.version, kubernetes_group_kind_version.kind)?;

							for (path, path_item, operation) in operations {
								write_operation(&mut file, operation, &replace_namespaces, mod_root, Some(&type_name), Some(&type_ref_path), path, path_item)?;
								num_generated_apis += 1;
							}

							writeln!(file)?;
							writeln!(file, "// End {}/{}/{}",
								kubernetes_group_kind_version.group, kubernetes_group_kind_version.version, kubernetes_group_kind_version.kind)?;
						}
					}
				}

				writeln!(file)?;
				writeln!(file, "impl<'de> ::serde::Deserialize<'de> for {} {{", type_name)?;
				writeln!(file, "    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: ::serde::Deserializer<'de> {{")?;
				writeln!(file, "        #[allow(non_camel_case_types)]")?;
				writeln!(file, "        enum Field {{")?;
				for Property { field_name, .. } in &properties {
					writeln!(file, "            Key_{},", field_name)?;
				}
				writeln!(file, "            Other,")?;
				writeln!(file, "        }}")?;
				writeln!(file)?;
				writeln!(file, "        impl<'de> ::serde::Deserialize<'de> for Field {{")?;
				writeln!(file, "            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: ::serde::Deserializer<'de> {{")?;
				writeln!(file, "                struct Visitor;")?;
				writeln!(file)?;
				writeln!(file, "                impl<'de> ::serde::de::Visitor<'de> for Visitor {{")?;
				writeln!(file, "                    type Value = Field;")?;
				writeln!(file)?;
				writeln!(file, "                    fn expecting(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {{")?;
				writeln!(file, r#"                        write!(f, "field identifier")"#)?;
				writeln!(file, "                    }}")?;
				writeln!(file)?;
				writeln!(file, "                    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> where E: ::serde::de::Error {{")?;
				writeln!(file, "                        Ok(match v {{")?;
				for Property { name, field_name, .. } in &properties {
					writeln!(file, r#"                            "{}" => Field::Key_{},"#, name, field_name)?;
				}
				writeln!(file, "                            _ => Field::Other,")?;
				writeln!(file, "                        }})")?;
				writeln!(file, "                    }}")?;
				writeln!(file, "                }}")?;
				writeln!(file)?;
				writeln!(file, "                deserializer.deserialize_identifier(Visitor)")?;
				writeln!(file, "            }}")?;
				writeln!(file, "        }}")?;
				writeln!(file)?;
				writeln!(file, "        struct Visitor;")?;
				writeln!(file)?;
				writeln!(file, "        impl<'de> ::serde::de::Visitor<'de> for Visitor {{")?;
				writeln!(file, "            type Value = {};", type_name)?;
				writeln!(file)?;
				writeln!(file, "            fn expecting(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {{")?;
				writeln!(file, r#"                write!(f, "struct {}")"#, type_name)?;
				writeln!(file, "            }}")?;
				writeln!(file)?;
				writeln!(file, "            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error> where A: ::serde::de::MapAccess<'de> {{")?;
				for Property { required, field_name, field_type_name, .. } in &properties {
					if *required {
						writeln!(file, r#"                let mut value_{}: Option<{}> = None;"#, field_name, field_type_name)?;
					}
					else {
						writeln!(file, r#"                let mut value_{}: {} = None;"#, field_name, field_type_name)?;
					}
				}
				writeln!(file)?;
				writeln!(file, "                while let Some(key) = ::serde::de::MapAccess::next_key::<Field>(&mut map)? {{")?;
				writeln!(file, "                    match key {{")?;
				for Property { required, field_name, .. } in &properties {
					if *required {
						writeln!(file, r#"                        Field::Key_{} => value_{} = Some(::serde::de::MapAccess::next_value(&mut map)?),"#, field_name, field_name)?;
					}
					else {
						writeln!(file, r#"                        Field::Key_{} => value_{} = ::serde::de::MapAccess::next_value(&mut map)?,"#, field_name, field_name)?;
					}
				}
				writeln!(file, "                        Field::Other => {{ let _: ::serde::de::IgnoredAny = ::serde::de::MapAccess::next_value(&mut map)?; }},")?;
				writeln!(file, "                    }}")?;
				writeln!(file, "                }}")?;
				writeln!(file)?;
				writeln!(file, "                Ok({} {{", type_name)?;
				for Property { name, required, field_name, .. } in &properties {
					if *required {
						writeln!(file, r#"                    {}: value_{}.ok_or_else(|| ::serde::de::Error::missing_field("{}"))?,"#, field_name, field_name, name)?;
					}
					else {
						writeln!(file, "                    {}: value_{},", field_name, field_name)?;
					}
				}
				writeln!(file, "                }})")?;
				writeln!(file, "            }}")?;
				writeln!(file, "        }}")?;
				writeln!(file)?;
				writeln!(file, "        deserializer.deserialize_struct(")?;
				writeln!(file, r#"            "{}","#, type_name)?;
				writeln!(file, "            &[")?;
				for Property { name, .. } in &properties {
					writeln!(file, r#"                "{}","#, name)?;
				}
				writeln!(file, "            ],")?;
				writeln!(file, "            Visitor,")?;
				writeln!(file, "        )")?;
				writeln!(file, "    }}")?;
				writeln!(file, "}}")?;
				writeln!(file)?;

				writeln!(file, "impl ::serde::Serialize for {} {{", type_name)?;
				writeln!(file, "    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: ::serde::Serializer {{")?;
				if properties.is_empty() {
					writeln!(file, "        let state = serializer.serialize_struct(")?;
				}
				else {
					writeln!(file, "        let mut state = serializer.serialize_struct(")?;
				}
				writeln!(file, r#"            "{}","#, type_name)?;
				write!(file, "            0")?;
				for Property { required, field_name, .. } in &properties {
					writeln!(file, " +")?;
					if *required {
						write!(file, "            1")?;
					}
					else {
						write!(file, "            self.{}.as_ref().map_or(0, |_| 1)", field_name)?;
					}
				}
				writeln!(file, ",")?;
				writeln!(file, "        )?;")?;
				for Property { name, required, field_name, .. } in &properties {
					if *required {
						writeln!(file, r#"        ::serde::ser::SerializeStruct::serialize_field(&mut state, "{}", &self.{})?;"#, name, field_name)?;
					}
					else {
						writeln!(file, "        if let Some(value) = &self.{} {{", field_name)?;
						writeln!(file, r#"            ::serde::ser::SerializeStruct::serialize_field(&mut state, "{}", value)?;"#, name)?;
						writeln!(file, "        }}")?;
					}
				}
				writeln!(file, "        ::serde::ser::SerializeStruct::end(state)")?;
				writeln!(file, "    }}")?;
				writeln!(file, "}}")?;

				num_generated_structs += 1;
			},

			swagger20::SchemaKind::Ref(_) => return Err(format!("{} is a Ref", definition_path).into()),

			swagger20::SchemaKind::Ty(swagger20::Type::IntOrString) => {
				writeln!(file, "#[derive(Clone, Debug, Eq, PartialEq)]")?;
				writeln!(file, "pub enum {} {{", type_name)?;
				writeln!(file, "    Int(i32),")?;
				writeln!(file, "    String(String),")?;
				writeln!(file, "}}")?;
				writeln!(file)?;
				writeln!(file, "impl Default for {} {{", type_name)?;
				writeln!(file, "    fn default() -> Self {{")?;
				writeln!(file, "        {}::Int(0)", type_name)?;
				writeln!(file, "    }}")?;
				writeln!(file, "}}")?;
				writeln!(file)?;
				writeln!(file, "impl<'de> ::serde::Deserialize<'de> for {} {{", type_name)?;
				writeln!(file, "    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: ::serde::Deserializer<'de> {{")?;
				writeln!(file, "        struct Visitor;")?;
				writeln!(file)?;
				writeln!(file, "        impl<'de> ::serde::de::Visitor<'de> for Visitor {{")?;
				writeln!(file, "            type Value = {};", type_name)?;
				writeln!(file)?;
				writeln!(file, "            fn expecting(&self, formatter: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {{")?;
				writeln!(file, r#"                write!(formatter, "enum {}")"#, type_name)?;
				writeln!(file, "            }}")?;
				writeln!(file)?;
				writeln!(file, "            fn visit_i32<E>(self, v: i32) -> Result<Self::Value, E> where E: ::serde::de::Error {{")?;
				writeln!(file, "                Ok({}::Int(v))", type_name)?;
				writeln!(file, "            }}")?;
				writeln!(file)?;
				writeln!(file, "            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E> where E: ::serde::de::Error {{")?;
				writeln!(file, "                if v < ::std::i32::MIN as i64 || v > ::std::i32::MAX as i64 {{")?;
				writeln!(file, r#"                    return Err(::serde::de::Error::invalid_value(::serde::de::Unexpected::Signed(v), &"a 32-bit integer"));"#)?;
				writeln!(file, "                }}")?;
				writeln!(file)?;
				writeln!(file, "                Ok({}::Int(v as i32))", type_name)?;
				writeln!(file, "            }}")?;
				writeln!(file)?;
				writeln!(file, "            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> where E: ::serde::de::Error {{")?;
				writeln!(file, "                if v > ::std::i32::MAX as u64 {{")?;
				writeln!(file, r#"                    return Err(::serde::de::Error::invalid_value(::serde::de::Unexpected::Unsigned(v), &"a 32-bit integer"));"#)?;
				writeln!(file, "                }}")?;
				writeln!(file)?;
				writeln!(file, "                Ok({}::Int(v as i32))", type_name)?;
				writeln!(file, "            }}")?;
				writeln!(file)?;
				writeln!(file, "            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> where E: ::serde::de::Error {{")?;
				writeln!(file, "                self.visit_string(v.to_string())")?;
				writeln!(file, "            }}")?;
				writeln!(file)?;
				writeln!(file, "            fn visit_string<E>(self, v: String) -> Result<Self::Value, E> where E: ::serde::de::Error {{")?;
				writeln!(file, "                Ok({}::String(v))", type_name)?;
				writeln!(file, "            }}")?;
				writeln!(file, "        }}")?;
				writeln!(file)?;
				writeln!(file, "        deserializer.deserialize_any(Visitor)")?;
				writeln!(file, "    }}")?;
				writeln!(file, "}}")?;
				writeln!(file)?;
				writeln!(file, "impl ::serde::Serialize for {} {{", type_name)?;
				writeln!(file, "    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: ::serde::Serializer {{")?;
				writeln!(file, "        match self {{")?;
				writeln!(file, "            {}::Int(i) => i.serialize(serializer),", type_name)?;
				writeln!(file, "            {}::String(s) => s.serialize(serializer),", type_name)?;
				writeln!(file, "        }}")?;
				writeln!(file, "    }}")?;
				writeln!(file, "}}")?;

				num_generated_structs += 1;
			},

			swagger20::SchemaKind::Ty(ty @ swagger20::Type::JSONSchemaPropsOrArray) |
			swagger20::SchemaKind::Ty(ty @ swagger20::Type::JSONSchemaPropsOrBool) |
			swagger20::SchemaKind::Ty(ty @ swagger20::Type::JSONSchemaPropsOrStringArray) => {
				let json_schema_props_type_name =
					get_fully_qualified_type_name(
						&swagger20::RefPath("io.k8s.apiextensions-apiserver.pkg.apis.apiextensions.v1beta1.JSONSchemaProps".to_string()),
						&replace_namespaces,
						mod_root)?;

				writeln!(file, "#[derive(Clone, Debug, PartialEq)]")?;
				writeln!(file, "pub enum {} {{", type_name)?;
				writeln!(file, "    Schema(Box<{}>),", json_schema_props_type_name)?; // Box to fix infinite recursion
				match ty {
					swagger20::Type::JSONSchemaPropsOrArray => writeln!(file, "    Schemas(Vec<{}>),", json_schema_props_type_name)?,
					swagger20::Type::JSONSchemaPropsOrBool => writeln!(file, "    Bool(bool),")?,
					swagger20::Type::JSONSchemaPropsOrStringArray => writeln!(file, "    Strings(Vec<String>),")?,
					_ => unreachable!(),
				}
				writeln!(file, "}}")?;
				writeln!(file)?;
				writeln!(file, "impl<'de> ::serde::Deserialize<'de> for {} {{", type_name)?;
				writeln!(file, "    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: ::serde::Deserializer<'de> {{")?;
				writeln!(file, "        struct Visitor;")?;
				writeln!(file)?;
				writeln!(file, "        impl<'de> ::serde::de::Visitor<'de> for Visitor {{")?;
				writeln!(file, "            type Value = {};", type_name)?;
				writeln!(file)?;
				writeln!(file, "            fn expecting(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {{")?;
				writeln!(file, r#"                write!(f, "enum {}")"#, type_name)?;
				writeln!(file, "            }}")?;
				writeln!(file)?;
				writeln!(file, "            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error> where A: ::serde::de::MapAccess<'de> {{")?;
				writeln!(file, "                Ok({}::Schema(::serde::de::Deserialize::deserialize(::serde::de::value::MapAccessDeserializer::new(map))?))", type_name)?;
				writeln!(file, "            }}")?;
				writeln!(file)?;

				match ty {
					swagger20::Type::JSONSchemaPropsOrArray => {
						writeln!(file, "            fn visit_seq<A>(self, seq: A) -> Result<Self::Value, A::Error> where A: ::serde::de::SeqAccess<'de> {{")?;
						writeln!(file, "                Ok({}::Schemas(::serde::de::Deserialize::deserialize(::serde::de::value::SeqAccessDeserializer::new(seq))?))", type_name)?;
						writeln!(file, "            }}")?;
					},

					swagger20::Type::JSONSchemaPropsOrBool => {
						writeln!(file, "            fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E> where E: ::serde::de::Error {{")?;
						writeln!(file, "                Ok({}::Bool(v))", type_name)?;
						writeln!(file, "            }}")?;
					},

					swagger20::Type::JSONSchemaPropsOrStringArray => {
						writeln!(file, "            fn visit_seq<A>(self, seq: A) -> Result<Self::Value, A::Error> where A: ::serde::de::SeqAccess<'de> {{")?;
						writeln!(file, "                Ok({}::Strings(::serde::de::Deserialize::deserialize(::serde::de::value::SeqAccessDeserializer::new(seq))?))", type_name)?;
						writeln!(file, "            }}")?;
					},

					_ => unreachable!(),
				}

				writeln!(file, "        }}")?;
				writeln!(file)?;
				writeln!(file, "        deserializer.deserialize_any(Visitor)")?;
				writeln!(file, "    }}")?;
				writeln!(file, "}}")?;
				writeln!(file)?;
				writeln!(file, "impl ::serde::Serialize for {} {{", type_name)?;
				writeln!(file, "    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: ::serde::Serializer {{")?;
				writeln!(file, "        match self {{")?;
				writeln!(file, "            {}::Schema(value) => value.serialize(serializer),", type_name)?;

				match ty {
					swagger20::Type::JSONSchemaPropsOrArray => writeln!(file, "            {}::Schemas(value) => value.serialize(serializer),", type_name)?,
					swagger20::Type::JSONSchemaPropsOrBool => writeln!(file, "            {}::Bool(value) => value.serialize(serializer),", type_name)?,
					swagger20::Type::JSONSchemaPropsOrStringArray => writeln!(file, "            {}::Strings(value) => value.serialize(serializer),", type_name)?,
					_ => unreachable!(),
				}

				writeln!(file, "        }}")?;
				writeln!(file, "    }}")?;
				writeln!(file, "}}")?;

				num_generated_structs += 1;
			},

			swagger20::SchemaKind::Ty(_) => {
				write!(file, "#[derive(Clone, Debug, ")?;
				if can_be_default {
					write!(file, "Default, ")?;
				}
				writeln!(file, "PartialEq)]")?;

				writeln!(file, "pub struct {}(pub {});", type_name, get_rust_type(&definition.kind, &replace_namespaces, mod_root)?)?;
				writeln!(file)?;
				writeln!(file, "impl<'de> ::serde::Deserialize<'de> for {} {{", type_name)?;
				writeln!(file, "    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: ::serde::Deserializer<'de> {{")?;
				writeln!(file, "        struct Visitor;")?;
				writeln!(file)?;
				writeln!(file, "        impl<'de> ::serde::de::Visitor<'de> for Visitor {{")?;
				writeln!(file, "            type Value = {};", type_name)?;
				writeln!(file)?;
				writeln!(file, "            fn expecting(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {{")?;
				writeln!(file, r#"                write!(f, "{}")"#, type_name)?;
				writeln!(file, "            }}")?;
				writeln!(file)?;
				writeln!(file, "            fn visit_newtype_struct<D>(self, deserializer: D) -> Result<Self::Value, D::Error> where D: ::serde::Deserializer<'de> {{")?;
				writeln!(file, "                Ok({}(::serde::Deserialize::deserialize(deserializer)?))", type_name)?;
				writeln!(file, "            }}")?;
				writeln!(file, "        }}")?;
				writeln!(file)?;
				writeln!(file, r#"        deserializer.deserialize_newtype_struct("{}", Visitor)"#, type_name)?;
				writeln!(file, "    }}")?;
				writeln!(file, "}}")?;
				writeln!(file)?;
				writeln!(file, "impl ::serde::Serialize for {} {{", type_name)?;
				writeln!(file, "    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: ::serde::Serializer {{")?;
				writeln!(file, r#"        serializer.serialize_newtype_struct("{}", &self.0)"#, type_name)?;
				writeln!(file, "    }}")?;
				writeln!(file, "}}")?;

				num_generated_type_aliases += 1;
			},
		}

		trace!("OK");
	}

	{
		let mut mod_root_file = std::io::BufWriter::new(std::fs::OpenOptions::new().append(true).open(out_dir.join("mod.rs"))?);

		for (kubernetes_group_kind_version, operations) in operations {
			for (path, path_item, operation) in operations {
				if let Some(swagger20::KubernetesGroupKindVersion { group, kind, version }) = kubernetes_group_kind_version {
					return Err(format!(
						"Operation {} is associated with {}/{}/{} but did not get emitted with that definition",
						operation.id, group, version, kind).into());
				}

				write_operation(&mut mod_root_file, operation, &replace_namespaces, mod_root, None, None, path, path_item)?;
				num_generated_apis += 1;
			}
		}
	}

	info!("OK");
	info!("Generated {} structs", num_generated_structs);
	info!("Generated {} type aliases", num_generated_type_aliases);
	info!("Generated {} API functions", num_generated_apis);

	if num_generated_structs + num_generated_type_aliases != expected_num_generated_types {
		return Err("Did not generate or skip expected number of types".into());
	}

	if num_generated_apis != expected_num_generated_apis {
		return Err("Did not generate expected number of API functions".into());
	}

	info!("");

	Ok(())
}

fn can_be_default(kind: &swagger20::SchemaKind, spec: &swagger20::Spec) -> Result<bool, Error> {
	match kind {
		swagger20::SchemaKind::Properties(properties) => {
			for (schema, required) in properties.values() {
				if !required {
					// Option<T>::default is None regardless of T
					continue;
				}

				if !can_be_default(&schema.kind, spec)? {
					return Ok(false);
				}
			}

			Ok(true)
		},

		swagger20::SchemaKind::Ref(ref_path) => {
			let target =
				spec.definitions.get(&swagger20::DefinitionPath(ref_path.0.clone()))
				.ok_or_else(|| format!("couldn't find target of ref path {}", ref_path))?;
			can_be_default(&target.kind, spec)
		},

		// chrono::DateTime<chrono::Utc> is not Default
		swagger20::SchemaKind::Ty(swagger20::Type::String { format: Some(swagger20::StringFormat::DateTime) }) => Ok(false),

		swagger20::SchemaKind::Ty(_) => Ok(true),
	}
}

fn create_file_for_type(
	definition_path: &swagger20::DefinitionPath,
	out_dir: &std::path::Path,
	replace_namespaces: &[(&[std::borrow::Cow<'static, str>], &[std::borrow::Cow<'static, str>])],
) -> Result<(std::io::BufWriter<std::fs::File>, String, swagger20::RefPath), Error> {
	use std::io::Write;

	let parts = replace_namespace(definition_path.split('.'), replace_namespaces);

	let mut current = out_dir.to_owned();

	for part in parts.iter().rev().skip(1).rev() {
		trace!("Current directory: {}", current.display());

		let mod_name = get_rust_ident(part);

		current.push(&*mod_name);

		trace!("Checking if subdirectory {} exists...", current.display());

		if !current.is_dir() {
			trace!("    Subdirectory does not exist. Creating mod.rs with a reference to it...");

			let current_mod_rs_path = current.with_file_name("mod.rs");
			let append_newline = current_mod_rs_path.exists();
			let mut parent_mod_rs = std::io::BufWriter::new(std::fs::OpenOptions::new().append(true).create(true).open(current_mod_rs_path)?);
			if append_newline {
				writeln!(parent_mod_rs)?;
			}
			writeln!(parent_mod_rs, "pub mod {};", mod_name)?;

			trace!("    OK");
			trace!("    Creating subdirectory...");

			std::fs::create_dir(&current)?;
			trace!("    OK");
		}

		trace!("OK");
	}

	let type_name = parts.last().ok_or_else(|| format!("path for {} has no parts", definition_path))?.to_string();

	let mod_name = get_rust_ident(&type_name);
	{
		let mut parent_mod_rs = std::io::BufWriter::new(std::fs::OpenOptions::new().append(true).create(true).open(current.join("mod.rs"))?);
		writeln!(parent_mod_rs)?;
		writeln!(parent_mod_rs, "mod {};", mod_name)?;
		writeln!(parent_mod_rs, "pub use self::{}::*;", mod_name)?;
	}

	let file_name = current.join(&*mod_name).with_extension("rs");
	let file = std::io::BufWriter::new(std::fs::File::create(file_name)?);

	let ref_path = swagger20::RefPath(definition_path.0.to_string());

	Ok((file, type_name, ref_path))
}

fn get_comment_text<'a>(s: &'a str, indent: &'a str) -> impl Iterator<Item = std::borrow::Cow<'static, str>> + 'a {
	s.lines().map(move |line|
		if line.is_empty() {
			"".into()
		}
		else {
			let line = line.replace("[", r"\[");
			let line = line.replace("]", r"\]");
			format!("{} {}", indent, line).into()
		})
}

fn get_fully_qualified_type_name(
	ref_path: &swagger20::RefPath,
	replace_namespaces: &[(&[std::borrow::Cow<'static, str>], &[std::borrow::Cow<'static, str>])],
	mod_root: &str,
) -> Result<String, Error> {
	use std::fmt::Write;

	let mut result = format!("::{}", mod_root);

	let parts = replace_namespace(ref_path.split('.'), replace_namespaces);

	for part in parts.iter().rev().skip(1).rev() {
		write!(result, "::{}", get_rust_ident(part))?;
	}

	write!(result, "::{}", parts.last().ok_or_else(|| format!("path for {} has no parts", ref_path))?)?;

	Ok(result)
}

fn get_rust_ident(name: &str) -> std::borrow::Cow<'static, str> {
	// Fix cases of invalid rust idents
	match name {
		"$ref" => return "ref_path".into(),
		"$schema" => return "schema".into(),
		"continue" => return "continue_".into(),
		"enum" => return "enum_".into(),
		"type" => return "type_".into(),
		_ => (),
	}

	// Some cases of "ABc" should be converted to "abc" instead of "a_bc".
	// Eg "JSONSchemas" => "json_schemas", but "externalIPs" => "external_ips" instead of "external_i_ps".
	// Mostly happens with plurals of abbreviations.
	match name {
		"externalIPs" => return "external_ips".into(),
		"nonResourceURLs" => return "non_resource_urls".into(),
		"serverAddressByClientCIDRs" => return "server_address_by_client_cidrs".into(),
		"targetWWNs" => return "target_wwns".into(),
		_ => (),
	}

	let mut result = String::new();

	let chars =
		name.chars()
		.zip(std::iter::once(None).chain(name.chars().map(|c| Some(c.is_uppercase()))))
		.zip(name.chars().skip(1).map(|c| Some(c.is_uppercase())).chain(std::iter::once(None)));

	for ((c, previous), next) in chars {
		if c.is_uppercase() {
			match (previous, next) {
				(Some(false), _) |
				(Some(true), Some(false)) => result.push('_'),
				_ => (),
			}

			result.extend(c.to_lowercase());
		}
		else {
			result.push(match c {
				'-' => '_',
				c => c,
			});
		}
	}

	result.into()
}

fn get_rust_borrow_type(
	schema_kind: &swagger20::SchemaKind,
	replace_namespaces: &[(&[std::borrow::Cow<'static, str>], &[std::borrow::Cow<'static, str>])],
	mod_root: &str,
) -> Result<std::borrow::Cow<'static, str>, Error> {
	match *schema_kind {
		swagger20::SchemaKind::Properties(_) => Err("Nested anonymous types not supported".into()),

		swagger20::SchemaKind::Ref(ref ref_path) => Ok(format!("&{}", get_fully_qualified_type_name(ref_path, replace_namespaces, mod_root)?).into()),

		swagger20::SchemaKind::Ty(swagger20::Type::Any) => Ok("&::serde_json::Value".into()),

		swagger20::SchemaKind::Ty(swagger20::Type::Array { ref items }) => Ok(format!("&[{}]", get_rust_type(&items.kind, replace_namespaces, mod_root)?).into()),

		swagger20::SchemaKind::Ty(swagger20::Type::Boolean) => Ok("bool".into()),

		swagger20::SchemaKind::Ty(swagger20::Type::Integer { format: swagger20::IntegerFormat::Int32 }) => Ok("i32".into()),
		swagger20::SchemaKind::Ty(swagger20::Type::Integer { format: swagger20::IntegerFormat::Int64 }) => Ok("i64".into()),

		swagger20::SchemaKind::Ty(swagger20::Type::Number { format: swagger20::NumberFormat::Double }) => Ok("f64".into()),

		swagger20::SchemaKind::Ty(swagger20::Type::Object { ref additional_properties }) =>
			Ok(format!("::std::collections::BTreeMap<String, {}>", get_rust_type(&additional_properties.kind, replace_namespaces, mod_root)?).into()),

		swagger20::SchemaKind::Ty(swagger20::Type::String { format: Some(swagger20::StringFormat::Byte) }) => Ok("&::ByteString".into()),
		swagger20::SchemaKind::Ty(swagger20::Type::String { format: Some(swagger20::StringFormat::DateTime) }) => Ok("&::chrono::DateTime<::chrono::Utc>".into()),
		swagger20::SchemaKind::Ty(swagger20::Type::String { format: None }) => Ok("&str".into()),

		swagger20::SchemaKind::Ty(swagger20::Type::IntOrString) => Err("nothing should be trying to refer to IntOrString".into()),

		swagger20::SchemaKind::Ty(swagger20::Type::JSONSchemaPropsOrArray) |
		swagger20::SchemaKind::Ty(swagger20::Type::JSONSchemaPropsOrBool) |
		swagger20::SchemaKind::Ty(swagger20::Type::JSONSchemaPropsOrStringArray) => Err("JSON schema types not supported".into()),
	}
}

fn get_rust_type(
	schema_kind: &swagger20::SchemaKind,
	replace_namespaces: &[(&[std::borrow::Cow<'static, str>], &[std::borrow::Cow<'static, str>])],
	mod_root: &str,
) -> Result<std::borrow::Cow<'static, str>, Error> {
	match *schema_kind {
		swagger20::SchemaKind::Properties(_) => Err("Nested anonymous types not supported".into()),

		swagger20::SchemaKind::Ref(ref ref_path) => Ok(get_fully_qualified_type_name(ref_path, replace_namespaces, mod_root)?.into()),

		swagger20::SchemaKind::Ty(swagger20::Type::Any) => Ok("::serde_json::Value".into()),

		swagger20::SchemaKind::Ty(swagger20::Type::Array { ref items }) => Ok(format!("Vec<{}>", get_rust_type(&items.kind, replace_namespaces, mod_root)?).into()),

		swagger20::SchemaKind::Ty(swagger20::Type::Boolean) => Ok("bool".into()),

		swagger20::SchemaKind::Ty(swagger20::Type::Integer { format: swagger20::IntegerFormat::Int32 }) => Ok("i32".into()),
		swagger20::SchemaKind::Ty(swagger20::Type::Integer { format: swagger20::IntegerFormat::Int64 }) => Ok("i64".into()),

		swagger20::SchemaKind::Ty(swagger20::Type::Number { format: swagger20::NumberFormat::Double }) => Ok("f64".into()),

		swagger20::SchemaKind::Ty(swagger20::Type::Object { ref additional_properties }) =>
			Ok(format!("::std::collections::BTreeMap<String, {}>", get_rust_type(&additional_properties.kind, replace_namespaces, mod_root)?).into()),

		swagger20::SchemaKind::Ty(swagger20::Type::String { format: Some(swagger20::StringFormat::Byte) }) => Ok("::ByteString".into()),
		swagger20::SchemaKind::Ty(swagger20::Type::String { format: Some(swagger20::StringFormat::DateTime) }) => Ok("::chrono::DateTime<::chrono::Utc>".into()),
		swagger20::SchemaKind::Ty(swagger20::Type::String { format: None }) => Ok("String".into()),

		swagger20::SchemaKind::Ty(swagger20::Type::IntOrString) => Err("nothing should be trying to refer to IntOrString".into()),

		swagger20::SchemaKind::Ty(swagger20::Type::JSONSchemaPropsOrArray) |
		swagger20::SchemaKind::Ty(swagger20::Type::JSONSchemaPropsOrBool) |
		swagger20::SchemaKind::Ty(swagger20::Type::JSONSchemaPropsOrStringArray) => Err("JSON schema types not supported".into()),
	}
}

fn replace_namespace<'a, I>(
	parts: I,
	replace_namespaces: &[(&[std::borrow::Cow<'static, str>], &[std::borrow::Cow<'static, str>])],
) -> Vec<std::borrow::Cow<'a, str>> where I: IntoIterator<Item = &'a str> {
	let parts: Vec<_> = parts.into_iter().map(Into::into).collect();

	trace!("parts = {:?}, replace_namespaces = {:?}", parts, replace_namespaces);

	for (from, to) in replace_namespaces {
		if parts.starts_with(from) {
			let mut result = to.to_vec();
			result.extend(parts.into_iter().skip(from.len()));
			return result;
		}
	}

	parts
}

fn write_operation(
	file: &mut std::io::BufWriter<std::fs::File>,
	operation: &swagger20::Operation,
	replace_namespaces: &[(&[std::borrow::Cow<'static, str>], &[std::borrow::Cow<'static, str>])],
	mod_root: &str,
	type_name: Option<&str>,
	type_ref_path: Option<&swagger20::RefPath>,
	path: &str,
	path_item: &swagger20::PathItem,
) -> Result<(), Error> {
	use std::io::Write;

	writeln!(file)?;

	writeln!(file, "// Generated from operation {}", operation.id)?;

	let operation_result_name = {
		let mut operation_id_chars = operation.id.chars();
		let first_operation_id_chars = operation_id_chars.next().ok_or_else(|| format!("operation has empty ID: {:?}", operation))?.to_uppercase();
		let rest_operation_id_chars = operation_id_chars.as_str();
		format!("{}{}Response", first_operation_id_chars, rest_operation_id_chars)
	};

	let operation_responses: Result<Vec<_>, _> =
		operation.responses.iter()
		.map(|(&status_code, schema)| {
			let http_status_code = match status_code {
				reqwest::StatusCode::ACCEPTED => "ACCEPTED",
				reqwest::StatusCode::CREATED => "CREATED",
				reqwest::StatusCode::OK => "OK",
				reqwest::StatusCode::UNAUTHORIZED => "UNAUTHORIZED",
				_ => return Err(format!("unrecognized status code {}", status_code)),
			};

			let variant_name = match status_code {
				reqwest::StatusCode::ACCEPTED => "Accepted",
				reqwest::StatusCode::CREATED => "Created",
				reqwest::StatusCode::OK => "Ok",
				reqwest::StatusCode::UNAUTHORIZED => "Unauthorized",
				_ => return Err(format!("unrecognized status code {}", status_code)),
			};

			let schema = schema.as_ref();

			let is_delete_ok_status = if let Some(schema) = schema {
				match &schema.kind {
					swagger20::SchemaKind::Ref(ref_path) if
						&**ref_path == "io.k8s.apimachinery.pkg.apis.meta.v1.Status" &&
						operation.method == swagger20::Method::Delete &&
						status_code == reqwest::StatusCode::OK => true,

					_ => false,
				}
			}
			else {
				false
			};

			Ok((http_status_code, variant_name, schema, is_delete_ok_status))
		})
		.collect();
	let operation_responses = operation_responses?;

	let indent = if type_name.is_some() { "    " } else { "" };

	writeln!(file)?;

	if let Some(type_name) = type_name {
		writeln!(file, "impl {} {{", type_name)?;
	}

	let operation_fn_name = get_rust_ident(&operation.id);

	let mut parameters: Vec<_> = path_item.parameters.iter().collect();
	for parameter in &operation.parameters {
		if let Some(p) = parameters.iter_mut().find(|p| p.name == parameter.name) {
			std::mem::replace(p, parameter);
			continue;
		}

		parameters.push(parameter);
	}
	let mut previous_parameters: std::collections::HashSet<_> = Default::default();
	let parameters: Result<Vec<_>, Error> =
		parameters.into_iter()
		.map(|parameter| {
			let mut parameter_name = get_rust_ident(&parameter.name);
			while previous_parameters.contains(&parameter_name) {
				parameter_name = format!("{}_", parameter_name).into();
			}
			previous_parameters.insert(parameter_name.clone());

			let parameter_type = get_rust_borrow_type(&parameter.schema.kind, replace_namespaces, mod_root)?;

			Ok((parameter_name, parameter_type, parameter))
		})
		.collect();
	let mut parameters = parameters?;
	parameters.sort_by(|(_, _, parameter1), (_, _, parameter2)| {
		(match (parameter1.location, parameter2.location) {
			(location1, location2) if location1 == location2 => std::cmp::Ordering::Equal,
			(swagger20::ParameterLocation::Path, _) |
			(swagger20::ParameterLocation::Body, swagger20::ParameterLocation::Query) => std::cmp::Ordering::Less,
			_ => std::cmp::Ordering::Greater,
		})
		.then_with(|| parameter1.name.cmp(&parameter2.name))
	});
	let parameters = parameters;

	let mut wrote_description = false;
	if let Some(description) = operation.description.as_ref() {
		for line in get_comment_text(description, "") {
			writeln!(file, "{}///{}", indent, line)?;
			wrote_description = true;
		}
	}

	if wrote_description {
		writeln!(file, "{}///", indent)?;
	}
	writeln!(file, "{}/// Use [`{}`](./enum.{}.html) to parse the HTTP response.", indent, operation_result_name, operation_result_name)?;

	if !parameters.is_empty() {
		writeln!(file, "{}///", indent)?;
		writeln!(file, "{}/// # Arguments", indent)?;
		for (parameter_name, _, parameter) in &parameters {
			writeln!(file, "{}///", indent)?;
			writeln!(file, "{}/// * `{}`", indent, parameter_name)?;
			if let Some(description) = parameter.schema.description.as_ref() {
				writeln!(file, "{}///", indent)?;
				for line in get_comment_text(description, "    ") {
					writeln!(file, "{}///{}", indent, line)?;
				}
			}
		}
	}

	writeln!(file, "{}pub fn {}(", indent, operation_fn_name)?;
	for (parameter_name, parameter_type, parameter) in &parameters {
		match (operation.method, parameter.location) {
			(swagger20::Method::Delete, swagger20::ParameterLocation::Body) |
			(swagger20::Method::Get, swagger20::ParameterLocation::Body) => continue,

			_ => (),
		}

		if parameter.required {
			writeln!(file, "{}    {}: {},", indent, parameter_name, parameter_type)?;
		}
		else {
			writeln!(file, "{}    {}: Option<{}>,", indent, parameter_name, parameter_type)?;
		}
	}
	writeln!(file, "{}) -> Result<::http::Request<Vec<u8>>, ::RequestError> {{", indent)?;

	let have_query_parameters = parameters.iter().any(|(_, _, parameter)| parameter.location == swagger20::ParameterLocation::Query);

	write!(file, r#"{}    let __url = format!("{}"#, indent, path)?;
	if have_query_parameters {
		write!(file, "?")?;
	}
	write!(file, r#"""#)?;
	for (parameter_name, _, parameter) in &parameters {
		if parameter.location == swagger20::ParameterLocation::Path {
			write!(file, ", {} = {}", parameter_name, parameter_name)?;
		}
	}
	writeln!(file, ");")?;

	if have_query_parameters {
		writeln!(file, "{}    let mut __query_pairs = ::url::form_urlencoded::Serializer::new(__url);", indent)?;
		for (parameter_name, parameter_type, parameter) in &parameters {
			if parameter.location == swagger20::ParameterLocation::Query {
				if parameter.required {
					match parameter.schema.kind {
						swagger20::SchemaKind::Ty(swagger20::Type::Boolean) |
						swagger20::SchemaKind::Ty(swagger20::Type::Integer { .. }) |
						swagger20::SchemaKind::Ty(swagger20::Type::Number { .. }) =>
							writeln!(file, r#"{}    __query_pairs.append_pair("{}", &{}.to_string());"#, indent, parameter.name, parameter_name)?,

						swagger20::SchemaKind::Ty(swagger20::Type::String { .. }) =>
							writeln!(file, r#"{}    __query_pairs.append_pair("{}", &{});"#, indent, parameter.name, parameter_name)?,

						_ => return Err(format!("parameter {} is in the query string but is a {:?}", parameter_name, parameter_type).into()),
					}
				}
				else {
					writeln!(file, "{}    if let Some({}) = {} {{", indent, parameter_name, parameter_name)?;
					match parameter.schema.kind {
						swagger20::SchemaKind::Ty(swagger20::Type::Boolean) |
						swagger20::SchemaKind::Ty(swagger20::Type::Integer { .. }) |
						swagger20::SchemaKind::Ty(swagger20::Type::Number { .. }) =>
							writeln!(file, r#"{}        __query_pairs.append_pair("{}", &{}.to_string());"#, indent, parameter.name, parameter_name)?,

						swagger20::SchemaKind::Ty(swagger20::Type::String { .. }) =>
							writeln!(file, r#"{}        __query_pairs.append_pair("{}", {});"#, indent, parameter.name, parameter_name)?,

						_ => return Err(format!("parameter {} is in the query string but is a {:?}", parameter_name, parameter_type).into()),
					}
					writeln!(file, "{}    }}", indent)?;
				}
			}
		}
		writeln!(file, "{}    let __url = __query_pairs.finish();", indent)?;
	}
	writeln!(file)?;

	let method = match operation.method {
		swagger20::Method::Delete => "delete",
		swagger20::Method::Get => "get",
		swagger20::Method::Patch => "patch",
		swagger20::Method::Post => "post",
		swagger20::Method::Put => "put",
	};

	writeln!(file, "{}    let mut __request = ::http::Request::{}(__url);", indent, method)?;

	let body_parameter = match operation.method {
		swagger20::Method::Delete | swagger20::Method::Get => None,

		swagger20::Method::Patch | swagger20::Method::Post | swagger20::Method::Put =>
			parameters.iter()
			.find(|(_, _, parameter)| parameter.location == swagger20::ParameterLocation::Body),
	};

	write!(file, "{}    let __body = ", indent)?;
	if let Some((parameter_name, _, parameter)) = body_parameter {
		if parameter.required {
			writeln!(file, "::serde_json::to_vec(&{}).map_err(::RequestError::Json)?;", parameter_name)?;
		}
		else {
			writeln!(file)?;
			writeln!(file, "{}.unwrap_or(Ok(vec![]), |value| ::serde_json::to_vec(value).map_err(::RequestError::Json))?;", parameter_name)?;
		}
	}
	else {
		writeln!(file, "vec![];")?;
	}

	writeln!(file, "{}    __request.body(__body).map_err(::RequestError::Http)", indent)?;
	writeln!(file, "{}}}", indent)?;

	if type_name.is_some() {
		writeln!(file, "}}")?;
	}

	writeln!(file)?;

	if let Some(type_name) = type_name {
		writeln!(file, "/// Parses the HTTP response of [`{}::{}`](./struct.{}.html#method.{})", type_name, operation_fn_name, type_name, operation_fn_name)?;
	}
	else {
		writeln!(file, "/// Parses the HTTP response of [`{}`](./fn.{}.html)", operation_fn_name, operation_fn_name)?;
	}

	writeln!(file, "#[derive(Debug)]")?;
	writeln!(file, "pub enum {} {{", operation_result_name)?;

	for &(_, variant_name, schema, is_delete_ok_status) in &operation_responses {
		if let Some(schema) = schema {
			if is_delete_ok_status {
				// DELETE operations that return metav1.Status for HTTP 200 can also return the object itself instead.
				//
				// Ref https://github.com/kubernetes/kubernetes/issues/59501
				writeln!(file, "    {}Status({}),", variant_name, get_rust_type(&schema.kind, replace_namespaces, mod_root)?)?;
				writeln!(file, "    {}Value({}),", variant_name, get_fully_qualified_type_name(
					type_ref_path.ok_or_else(|| "DELETE-Ok-Status that isn't associated with a type")?,
					&replace_namespaces,
					mod_root)?)?;
			}
			else {
				writeln!(file, "    {}({}),", variant_name, get_rust_type(&schema.kind, replace_namespaces, mod_root)?)?;
			}
		}
		else {
			writeln!(file, "    {},", variant_name)?;
		}
	}
	writeln!(file, "    Other,")?;
	writeln!(file, "}}")?;
	writeln!(file)?;

	writeln!(file, "impl ::Response for {} {{", operation_result_name)?;

	let uses_buf = operation_responses.iter().any(|&(_, _, schema, _)| schema.is_some());

	if uses_buf {
		writeln!(file, "    fn try_from_parts(status_code: ::http::StatusCode, buf: &[u8]) -> Result<(Self, usize), ::ResponseError> {{")?;
	}
	else {
		writeln!(file, "    fn try_from_parts(status_code: ::http::StatusCode, _: &[u8]) -> Result<(Self, usize), ::ResponseError> {{")?;
	}

	let is_watch = match operation.kubernetes_action {
		Some(swagger20::KubernetesAction::Watch) | Some(swagger20::KubernetesAction::WatchList) => true,
		_ => false,
	};

	writeln!(file, "        match status_code {{")?;
	for &(http_status_code, variant_name, schema, is_delete_ok_status) in &operation_responses {
		write!(file, "            ::http::StatusCode::{} => ", http_status_code)?;
		if let Some(schema) = schema {
			writeln!(file, "{{")?;

			match &schema.kind {
				swagger20::SchemaKind::Ty(swagger20::Type::String { .. }) => {
					writeln!(file, "                let result = match ::std::str::from_utf8(buf) {{")?;
					writeln!(file, "                    Ok(s) => s,")?;
					writeln!(file, "                    Err(err) if err.error_len().is_none() => {{")?;
					writeln!(file, "                        let valid_up_to = err.valid_up_to();")?;
					writeln!(file, "                        unsafe {{ ::std::str::from_utf8_unchecked(&buf[..valid_up_to]) }}")?;
					writeln!(file, "                    }},")?;
					writeln!(file, "                    Err(err) => return Err(::ResponseError::Utf8(err)),")?;
					writeln!(file, "                }};")?;
					writeln!(file, "                let result = result.to_string();")?;
					writeln!(file, "                let len = result.len();")?;
					writeln!(file, "                Ok(({}::{}(result), len))", operation_result_name, variant_name)?;
				},

				swagger20::SchemaKind::Ref(_) => if is_watch {
					writeln!(file, "                let mut deserializer = ::serde_json::Deserializer::from_slice(buf).into_iter();")?;
					writeln!(file, "                let (result, byte_offset) = match deserializer.next() {{")?;
					writeln!(file, "                    Some(Ok(value)) => (value, deserializer.byte_offset()),")?;
					writeln!(file, "                    Some(Err(ref err)) if err.is_eof() => return Err(::ResponseError::NeedMoreData),")?;
					writeln!(file, "                    Some(Err(err)) => return Err(::ResponseError::Json(err)),")?;
					writeln!(file, "                    None => return Err(::ResponseError::NeedMoreData),")?;
					writeln!(file, "                }};")?;
					writeln!(file, "                Ok(({}::{}(result), byte_offset))", operation_result_name, variant_name)?;
				}
				else if is_delete_ok_status {
					writeln!(file, "                let result: ::serde_json::Map<String, ::serde_json::Value> = match ::serde_json::from_slice(buf) {{")?;
					writeln!(file, "                    Ok(value) => value,")?;
					writeln!(file, "                    Err(ref err) if err.is_eof() => return Err(::ResponseError::NeedMoreData),")?;
					writeln!(file, "                    Err(err) => return Err(::ResponseError::Json(err)),")?;
					writeln!(file, "                }};")?;
					writeln!(file, r#"                let is_status = match result.get("kind") {{"#)?;
					writeln!(file, r#"                    Some(::serde_json::Value::String(s)) if s == "Status" => true,"#)?;
					writeln!(file, "                    _ => false,")?;
					writeln!(file, "                }};")?;
					writeln!(file, "                if is_status {{")?;
					writeln!(file, "                    let result = ::serde::Deserialize::deserialize(::serde_json::Value::Object(result));")?;
					writeln!(file, "                    let result = result.map_err(::ResponseError::Json)?;")?;
					writeln!(file, "                    Ok(({}::{}Status(result), buf.len()))", operation_result_name, variant_name)?;
					writeln!(file, "                }}")?;
					writeln!(file, "                else {{")?;
					writeln!(file, "                    let result = ::serde::Deserialize::deserialize(::serde_json::Value::Object(result));")?;
					writeln!(file, "                    let result = result.map_err(::ResponseError::Json)?;")?;
					writeln!(file, "                    Ok(({}::{}Value(result), buf.len()))", operation_result_name, variant_name)?;
					writeln!(file, "                }}")?;
				}
				else {
					writeln!(file, "                let result = match ::serde_json::from_slice(buf) {{")?;
					writeln!(file, "                    Ok(value) => value,")?;
					writeln!(file, "                    Err(ref err) if err.is_eof() => return Err(::ResponseError::NeedMoreData),")?;
					writeln!(file, "                    Err(err) => return Err(::ResponseError::Json(err)),")?;
					writeln!(file, "                }};")?;
					writeln!(file, "                Ok(({}::{}(result), buf.len()))", operation_result_name, variant_name)?;
				},

				other => return Err(format!("operation {} has unrecognized type for response of variant {}: {:?}", operation.id, variant_name, other).into()),
			}

			writeln!(file, "            }},")?;
		}
		else {
			writeln!(file, "Ok(({}::{}, 0)),", operation_result_name, variant_name)?;
		}
	}
	writeln!(file, "            _ => Ok(({}::Other, 0)),", operation_result_name)?;
	writeln!(file, "        }}")?;
	writeln!(file, "    }}")?;
	writeln!(file, "}}")?;

	Ok(())
}
