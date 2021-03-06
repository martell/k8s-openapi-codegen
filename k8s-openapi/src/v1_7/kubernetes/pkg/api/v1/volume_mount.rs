// Generated from definition io.k8s.kubernetes.pkg.api.v1.VolumeMount

/// VolumeMount describes a mounting of a Volume within a container.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VolumeMount {
    /// Path within the container at which the volume should be mounted.  Must not contain ':'.
    pub mount_path: String,

    /// This must match the Name of a Volume.
    pub name: String,

    /// Mounted read-only if true, read-write otherwise (false or unspecified). Defaults to false.
    pub read_only: Option<bool>,

    /// Path within the volume from which the container's volume should be mounted. Defaults to "" (volume's root).
    pub sub_path: Option<String>,
}

impl<'de> ::serde::Deserialize<'de> for VolumeMount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: ::serde::Deserializer<'de> {
        #[allow(non_camel_case_types)]
        enum Field {
            Key_mount_path,
            Key_name,
            Key_read_only,
            Key_sub_path,
            Other,
        }

        impl<'de> ::serde::Deserialize<'de> for Field {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: ::serde::Deserializer<'de> {
                struct Visitor;

                impl<'de> ::serde::de::Visitor<'de> for Visitor {
                    type Value = Field;

                    fn expecting(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                        write!(f, "field identifier")
                    }

                    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> where E: ::serde::de::Error {
                        Ok(match v {
                            "mountPath" => Field::Key_mount_path,
                            "name" => Field::Key_name,
                            "readOnly" => Field::Key_read_only,
                            "subPath" => Field::Key_sub_path,
                            _ => Field::Other,
                        })
                    }
                }

                deserializer.deserialize_identifier(Visitor)
            }
        }

        struct Visitor;

        impl<'de> ::serde::de::Visitor<'de> for Visitor {
            type Value = VolumeMount;

            fn expecting(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                write!(f, "struct VolumeMount")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error> where A: ::serde::de::MapAccess<'de> {
                let mut value_mount_path: Option<String> = None;
                let mut value_name: Option<String> = None;
                let mut value_read_only: Option<bool> = None;
                let mut value_sub_path: Option<String> = None;

                while let Some(key) = ::serde::de::MapAccess::next_key::<Field>(&mut map)? {
                    match key {
                        Field::Key_mount_path => value_mount_path = Some(::serde::de::MapAccess::next_value(&mut map)?),
                        Field::Key_name => value_name = Some(::serde::de::MapAccess::next_value(&mut map)?),
                        Field::Key_read_only => value_read_only = ::serde::de::MapAccess::next_value(&mut map)?,
                        Field::Key_sub_path => value_sub_path = ::serde::de::MapAccess::next_value(&mut map)?,
                        Field::Other => { let _: ::serde::de::IgnoredAny = ::serde::de::MapAccess::next_value(&mut map)?; },
                    }
                }

                Ok(VolumeMount {
                    mount_path: value_mount_path.ok_or_else(|| ::serde::de::Error::missing_field("mountPath"))?,
                    name: value_name.ok_or_else(|| ::serde::de::Error::missing_field("name"))?,
                    read_only: value_read_only,
                    sub_path: value_sub_path,
                })
            }
        }

        deserializer.deserialize_struct(
            "VolumeMount",
            &[
                "mountPath",
                "name",
                "readOnly",
                "subPath",
            ],
            Visitor,
        )
    }
}

impl ::serde::Serialize for VolumeMount {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: ::serde::Serializer {
        let mut state = serializer.serialize_struct(
            "VolumeMount",
            0 +
            1 +
            1 +
            self.read_only.as_ref().map_or(0, |_| 1) +
            self.sub_path.as_ref().map_or(0, |_| 1),
        )?;
        ::serde::ser::SerializeStruct::serialize_field(&mut state, "mountPath", &self.mount_path)?;
        ::serde::ser::SerializeStruct::serialize_field(&mut state, "name", &self.name)?;
        if let Some(value) = &self.read_only {
            ::serde::ser::SerializeStruct::serialize_field(&mut state, "readOnly", value)?;
        }
        if let Some(value) = &self.sub_path {
            ::serde::ser::SerializeStruct::serialize_field(&mut state, "subPath", value)?;
        }
        ::serde::ser::SerializeStruct::end(state)
    }
}
