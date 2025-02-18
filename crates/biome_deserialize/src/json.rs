//! Implementation of [DeserializableValue] for the JSON data format.
use crate::{
    Deserializable, DeserializableValue, DeserializationDiagnostic, DeserializationVisitor,
    Deserialized, Text, TextNumber,
};
use biome_diagnostics::{DiagnosticExt, Error};
use biome_json_parser::{parse_json, JsonParserOptions};
use biome_json_syntax::{AnyJsonValue, JsonMemberName, JsonRoot, T};
use biome_rowan::{AstNode, AstSeparatedList};

/// It attempts to parse and deserialize a source file in JSON. Diagnostics from the parse phase
/// are consumed and joined with the diagnostics emitted during the deserialization.
///
/// The data structures that need to be deserialized have to implement the [Deserializable] trait.
/// To implement [Deserializable], it can need to implement [DeserializationVisitor] that allows
/// visiting a value.
///
/// ## Examples
///
/// ```
/// use biome_deserialize::{DeserializationDiagnostic, Deserializable, DeserializableValue, DeserializationVisitor, Text, VisitableType};
/// use biome_deserialize::json::deserialize_from_json_str;
/// use biome_json_parser::JsonParserOptions;
/// use biome_rowan::{TextRange, TokenText};
///
/// #[derive(Default, Debug, Eq, PartialEq)]
/// struct NewConfiguration {
///     lorem: String
/// }
///
/// impl Deserializable for NewConfiguration {
///     fn deserialize(
///         value: &impl DeserializableValue,
///         name: &str,
///         diagnostics: &mut Vec<DeserializationDiagnostic>,
///     ) -> Option<Self> {
///         value.deserialize(Visitor, name, diagnostics)
///     }
/// }
///
/// struct Visitor;
/// impl DeserializationVisitor for Visitor {
///     type Output = NewConfiguration;
///
///     const EXPECTED_TYPE: VisitableType = VisitableType::MAP;
///
///     fn visit_map(
///         self,
///         members: impl Iterator<Item = Option<(impl DeserializableValue, impl DeserializableValue)>>,
///         _range: TextRange,
///         _name: &str,
///         diagnostics: &mut Vec<DeserializationDiagnostic>,
///     ) -> Option<Self::Output> {
///         const ALLOWED_KEYS: &[&str] = &["strictCase", "enumMemberCase"];
///         let mut result = NewConfiguration::default();
///         for (key, value) in members.flatten() {
///             let Some(key_text) = Text::deserialize(&key, "", diagnostics) else {
///                 continue;
///             };
///             match key_text.text() {
///                 "lorem" => {
///                     if let Some(value) = Deserializable::deserialize(&value, &key_text, diagnostics) {
///                         result.lorem = value;
///                     }
///                 },
///                 _ => diagnostics.push(DeserializationDiagnostic::new_unknown_key(
///                     &key_text,
///                     key.range(),
///                     ALLOWED_KEYS,
///                 )),
///             }
///         }
///         Some(result)
///     }
/// }
///
/// let source = r#"{ "lorem": "ipsum" }"#;
/// let deserialized = deserialize_from_json_str::<NewConfiguration>(&source, JsonParserOptions::default());
/// assert!(!deserialized.has_errors());
/// assert_eq!(deserialized.into_deserialized().unwrap(), NewConfiguration { lorem: "ipsum".to_string() });
/// ```
pub fn deserialize_from_json_str<Output: Deserializable>(
    source: &str,
    options: JsonParserOptions,
) -> Deserialized<Output> {
    let parse = parse_json(source, options);
    let Deserialized {
        diagnostics,
        deserialized,
    } = deserialize_from_json_ast::<Output>(&parse.tree());
    let mut errors = parse
        .into_diagnostics()
        .into_iter()
        .map(Error::from)
        .collect::<Vec<_>>();
    errors.extend(
        diagnostics
            .into_iter()
            .map(|diagnostic| diagnostic.with_file_source_code(source))
            .collect::<Vec<_>>(),
    );
    Deserialized {
        diagnostics: errors,
        deserialized,
    }
}

/// Attempts to deserialize a JSON AST, given the `Output`.
pub fn deserialize_from_json_ast<Output: Deserializable>(parse: &JsonRoot) -> Deserialized<Output> {
    let mut diagnostics = vec![];
    let deserialized = parse
        .value()
        .ok()
        .and_then(|value| Output::deserialize(&value, "", &mut diagnostics));
    Deserialized {
        diagnostics: diagnostics.into_iter().map(Error::from).collect::<Vec<_>>(),
        deserialized,
    }
}

impl DeserializableValue for AnyJsonValue {
    fn range(&self) -> biome_rowan::TextRange {
        AstNode::range(self)
    }

    fn deserialize<V: DeserializationVisitor>(
        &self,
        visitor: V,
        name: &str,
        diagnostics: &mut Vec<DeserializationDiagnostic>,
    ) -> Option<V::Output> {
        let range = AstNode::range(self);
        match self {
            AnyJsonValue::JsonArrayValue(array) => {
                let items = array.elements().iter().map(|x| x.ok());
                visitor.visit_array(items, range, name, diagnostics)
            }
            AnyJsonValue::JsonBogusValue(_) => {
                // The parser should emit an error about this node
                // No need to emit another diagnostic.
                None
            }
            AnyJsonValue::JsonBooleanValue(value) => {
                let value = value.value_token().ok()?;
                visitor.visit_bool(value.kind() == T![true], range, name, diagnostics)
            }
            AnyJsonValue::JsonNullValue(_) => visitor.visit_null(range, name, diagnostics),
            AnyJsonValue::JsonNumberValue(value) => {
                let value = value.value_token().ok()?;
                let token_text = value.token_text_trimmed();
                visitor.visit_number(TextNumber(token_text), range, name, diagnostics)
            }
            AnyJsonValue::JsonObjectValue(object) => {
                let members = object.json_member_list().iter().map(|member| {
                    let member = member.ok()?;
                    Some((member.name().ok()?, member.value().ok()?))
                });
                visitor.visit_map(members, range, name, diagnostics)
            }
            AnyJsonValue::JsonStringValue(value) => {
                let value = value.inner_string_text().ok()?;
                visitor.visit_str(Text(value), range, name, diagnostics)
            }
        }
    }
}

impl DeserializableValue for JsonMemberName {
    fn range(&self) -> biome_rowan::TextRange {
        AstNode::range(self)
    }

    fn deserialize<V: DeserializationVisitor>(
        &self,
        visitor: V,
        name: &str,
        diagnostics: &mut Vec<DeserializationDiagnostic>,
    ) -> Option<V::Output> {
        let value = self.inner_string_text().ok()?;
        visitor.visit_str(Text(value), AstNode::range(self), name, diagnostics)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, HashMap, HashSet},
        num::{NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU8, NonZeroUsize},
    };

    use super::*;
    use biome_json_parser::JsonParserOptions;
    use indexmap::{IndexMap, IndexSet};

    #[test]
    fn test_unit() {
        let source = "0";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<()>(source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_bool() {
        let source = "true";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<bool>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert!(deserialized.unwrap());

        let source = "0";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<bool>(source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_f32() {
        let source = "0.5";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<f32>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized, Some(0.5));
    }

    #[test]
    fn test_f64() {
        let source = "0.5";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<f64>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized, Some(0.5));
    }

    #[test]
    fn test_i8() {
        let source = "-1";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<i8>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized, Some(-1));

        let source = u8::MAX.to_string();
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<i8>(&source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_i16() {
        let source = "-1";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<i16>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized, Some(-1));

        let source = u16::MAX.to_string();
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<i16>(&source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_i32() {
        let source = "-1";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<i32>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized, Some(-1));

        let source = u32::MAX.to_string();
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<i32>(&source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_i64() {
        let source = "-1";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<i64>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized, Some(-1));

        let source = u64::MAX.to_string();
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<i64>(&source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_isize() {
        let source = "-1";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<isize>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized, Some(-1));

        let source = usize::MAX.to_string();
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<isize>(&source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_u8() {
        let source = "0";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<u8>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized, Some(0));

        let source = "256";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<u8>(source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_u16() {
        let source = "0";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<u16>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized, Some(0));

        let source = "-1";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<u16>(source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_u32() {
        let source = "0";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<u32>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized, Some(0));

        let source = "-1";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<u32>(source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_u64() {
        let source = "0";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<u64>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized, Some(0));

        let source = "-1";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<u64>(source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_usize() {
        let source = "0";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<usize>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized, Some(0));

        let source = "-1";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<usize>(source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_non_zero_u8() {
        let source = "1";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<NonZeroU8>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized, NonZeroU8::new(1));

        let source = "0";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<NonZeroU8>(source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_non_zero_u16() {
        let source = "1";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<NonZeroU16>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized, NonZeroU16::new(1));

        let source = "0";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<NonZeroU16>(source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_non_zero_u32() {
        let source = "1";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<NonZeroU32>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized, NonZeroU32::new(1));

        let source = "0";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<NonZeroU32>(source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_non_zero_u64() {
        let source = "1";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<NonZeroU64>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized, NonZeroU64::new(1));

        let source = "0";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<NonZeroU64>(source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_non_zero_usize() {
        let source = "1";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<NonZeroUsize>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized, NonZeroUsize::new(1));

        let source = "0";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<NonZeroUsize>(source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_number() {
        let source = u128::MAX.to_string();
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<TextNumber>(&source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized.unwrap().text(), u128::MAX.to_string());

        let source = "true";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<TextNumber>(source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_string() {
        let source = r#""string""#;
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<String>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized.unwrap(), "string");

        let source = "0";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<String>(source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_vec() {
        let source = r#"[0, 1]"#;
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<Vec<u8>>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized.unwrap(), vec![0, 1]);

        let source = "0";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<Vec<u8>>(source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_hash_set() {
        let source = r#"[0, 1]"#;
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<HashSet<u8>>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized.unwrap(), HashSet::from([0, 1]));

        let source = "0";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<HashSet<u8>>(source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_index_set() {
        let source = r#"[0, 1]"#;
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<IndexSet<u8>>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(deserialized.unwrap(), IndexSet::from([0, 1]));

        let source = "0";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<IndexSet<u8>>(source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_hash_map() {
        let source = r#"{ "a": 0, "b": 1 }"#;
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<HashMap<String, u8>>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(
            deserialized.unwrap(),
            HashMap::from([("a".to_string(), 0), ("b".to_string(), 1)])
        );

        let source = "0";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<HashMap<String, u8>>(source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_b_tree_map_map() {
        let source = r#"{ "a": 0, "b": 1 }"#;
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<BTreeMap<String, u8>>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(
            deserialized.unwrap(),
            BTreeMap::from([("a".to_string(), 0), ("b".to_string(), 1)])
        );

        let source = "0";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<BTreeMap<String, u8>>(source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }

    #[test]
    fn test_index_map() {
        let source = r#"{ "a": 0, "b": 1 }"#;
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<IndexMap<String, u8>>(source, JsonParserOptions::default());
        assert!(diagnostics.is_empty());
        assert_eq!(
            deserialized.unwrap(),
            IndexMap::from([("a".to_string(), 0), ("b".to_string(), 1)])
        );

        let source = "0";
        let Deserialized {
            deserialized,
            diagnostics,
        } = deserialize_from_json_str::<IndexMap<String, u8>>(source, JsonParserOptions::default());
        assert!(!diagnostics.is_empty());
        assert!(deserialized.is_none());
    }
}
