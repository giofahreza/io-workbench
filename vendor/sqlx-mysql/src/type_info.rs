use std::fmt::{self, Display, Formatter};

pub(crate) use sqlx_core::type_info::*;

use crate::protocol::text::{ColumnDefinition, ColumnFlags, ColumnType};

/// Type information for a MySql type.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "offline", derive(serde::Serialize, serde::Deserialize))]
pub struct MySqlTypeInfo {
    pub(crate) r#type: ColumnType,
    pub(crate) flags: ColumnFlags,

    // [max_size] for integer types, this is (M) in BIT(M) or TINYINT(M)
    #[cfg_attr(feature = "offline", serde(default))]
    pub(crate) max_size: Option<u32>,
}

impl MySqlTypeInfo {
    pub(crate) const fn binary(ty: ColumnType) -> Self {
        Self {
            r#type: ty,
            flags: ColumnFlags::BINARY,
            max_size: None,
        }
    }

    #[doc(hidden)]
    pub const fn __enum() -> Self {
        Self {
            r#type: ColumnType::Enum,
            flags: ColumnFlags::BINARY,
            max_size: None,
        }
    }

    #[doc(hidden)]
    pub fn __type_feature_gate(&self) -> Option<&'static str> {
        match self.r#type {
            ColumnType::Date | ColumnType::Time | ColumnType::Timestamp | ColumnType::Datetime => {
                Some("time")
            }

            ColumnType::Json => Some("json"),
            ColumnType::NewDecimal => Some("bigdecimal"),

            _ => None,
        }
    }

    pub(crate) fn from_column(column: &ColumnDefinition) -> Self {
        Self {
            r#type: column.r#type,
            flags: type_info_flags(column.r#type, column.flags, column.collation),
            max_size: Some(column.max_size),
        }
    }
}

// The protocol BINARY flag also marks textual *_bin collations. Preserve its
// binary-value meaning in TypeInfo only when the wire charset is binary/63.
// ColumnDefinition and MySqlColumn retain the original flags; storage collation
// and database behavior are unchanged. This makes both name() and the existing
// String/byte compatibility checks reflect the actual wire representation.
fn type_info_flags(kind: ColumnType, mut flags: ColumnFlags, collation: u16) -> ColumnFlags {
    if collation != 63
        && matches!(
            kind,
            ColumnType::VarChar
                | ColumnType::Blob
                | ColumnType::TinyBlob
                | ColumnType::MediumBlob
                | ColumnType::LongBlob
                | ColumnType::String
                | ColumnType::VarString
                | ColumnType::Enum
                | ColumnType::Set
        )
    {
        flags.remove(ColumnFlags::BINARY);
    }
    flags
}

impl Display for MySqlTypeInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.pad(self.name())
    }
}

impl TypeInfo for MySqlTypeInfo {
    fn is_null(&self) -> bool {
        matches!(self.r#type, ColumnType::Null)
    }

    fn name(&self) -> &str {
        self.r#type.name(self.flags, self.max_size)
    }
}

impl PartialEq<MySqlTypeInfo> for MySqlTypeInfo {
    fn eq(&self, other: &MySqlTypeInfo) -> bool {
        if self.r#type != other.r#type {
            return false;
        }

        match self.r#type {
            ColumnType::Tiny
            | ColumnType::Short
            | ColumnType::Long
            | ColumnType::Int24
            | ColumnType::LongLong => {
                return self.flags.contains(ColumnFlags::UNSIGNED)
                    == other.flags.contains(ColumnFlags::UNSIGNED);
            }

            // for string types, check that our charset matches
            ColumnType::VarChar
            | ColumnType::Blob
            | ColumnType::TinyBlob
            | ColumnType::MediumBlob
            | ColumnType::LongBlob
            | ColumnType::String
            | ColumnType::VarString
            | ColumnType::Enum => {
                return self.flags == other.flags;
            }
            _ => {}
        }

        true
    }
}

impl Eq for MySqlTypeInfo {}

#[cfg(test)]
mod binary_collation_tests {
    use super::*;
    use crate::types::Type;
    use crate::MySql;

    fn info(kind: ColumnType, collation: u16) -> MySqlTypeInfo {
        MySqlTypeInfo {
            r#type: kind,
            flags: type_info_flags(kind, ColumnFlags::BINARY | ColumnFlags::NOT_NULL, collation),
            max_size: None,
        }
    }

    #[test]
    fn binary_collated_text_uses_text_names_and_string_compatibility() {
        for collation in [45, 46, 255] {
            for (kind, name) in [
                (ColumnType::VarString, "VARCHAR"),
                (ColumnType::String, "CHAR"),
                (ColumnType::Blob, "TEXT"),
                (ColumnType::LongBlob, "LONGTEXT"),
            ] {
                let value = info(kind, collation);
                assert_eq!(value.name(), name);
                assert!(<String as Type<MySql>>::compatible(&value));
                assert!(value.flags.contains(ColumnFlags::NOT_NULL));
            }
        }
    }

    #[test]
    fn actual_binary_charset_keeps_bytes_and_nontext_flags_are_unchanged() {
        for (kind, name) in [
            (ColumnType::VarString, "VARBINARY"),
            (ColumnType::String, "BINARY"),
            (ColumnType::Blob, "BLOB"),
            (ColumnType::LongBlob, "LONGBLOB"),
        ] {
            let value = info(kind, 63);
            assert_eq!(value.name(), name);
            assert!(!<String as Type<MySql>>::compatible(&value));
            assert!(value.flags.contains(ColumnFlags::BINARY));
        }
        for kind in [
            ColumnType::Json,
            ColumnType::Geometry,
            ColumnType::Bit,
            ColumnType::Long,
        ] {
            let flags = ColumnFlags::BINARY | ColumnFlags::UNSIGNED | ColumnFlags::NOT_NULL;
            assert_eq!(type_info_flags(kind, flags, 45), flags);
            assert_eq!(type_info_flags(kind, flags, 63), flags);
        }
    }
}
