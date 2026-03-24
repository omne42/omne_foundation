#![forbid(unsafe_code)]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use structured_text_kit::{
    CatalogArgValueRef, CatalogText, CatalogTextRef, StructuredText, StructuredTextRef,
    StructuredTextValidationError,
};
use thiserror::Error;
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind")]
#[ts(export, rename_all = "snake_case")]
pub enum StructuredTextData {
    #[serde(rename = "catalog")]
    Catalog {
        code: String,
        args: Vec<CatalogArgData>,
    },
    #[serde(rename = "freeform")]
    Freeform { text: String },
}

impl StructuredTextData {
    #[must_use]
    pub fn catalog_code(&self) -> Option<&str> {
        match self {
            Self::Catalog { code, .. } => Some(code.as_str()),
            Self::Freeform { .. } => None,
        }
    }

    #[must_use]
    pub fn freeform_text(&self) -> Option<&str> {
        match self {
            Self::Catalog { .. } => None,
            Self::Freeform { text } => Some(text.as_str()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct CatalogArgData {
    pub name: String,
    pub value: CatalogArgValueData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", content = "value")]
#[ts(export, rename_all = "snake_case")]
pub enum CatalogArgValueData {
    #[serde(rename = "text")]
    Text(String),
    #[serde(rename = "bool")]
    Bool(bool),
    #[serde(rename = "signed")]
    Signed(String),
    #[serde(rename = "unsigned")]
    Unsigned(String),
    #[serde(rename = "nested_text")]
    NestedText(Box<StructuredTextData>),
}

#[derive(Debug, Error)]
pub enum StructuredTextDataError {
    #[error(transparent)]
    Validation(#[from] StructuredTextValidationError),
    #[error("catalog arg `{arg_name}` has invalid signed integer `{value}`")]
    InvalidSignedInteger { arg_name: String, value: String },
    #[error("catalog arg `{arg_name}` has invalid unsigned integer `{value}`")]
    InvalidUnsignedInteger { arg_name: String, value: String },
}

impl From<&StructuredText> for StructuredTextData {
    fn from(text: &StructuredText) -> Self {
        Self::from(text.text_ref())
    }
}

impl From<StructuredTextRef<'_>> for StructuredTextData {
    fn from(text: StructuredTextRef<'_>) -> Self {
        match text {
            StructuredTextRef::Catalog(text) => Self::from(text),
            StructuredTextRef::Freeform(text) => Self::Freeform {
                text: text.to_owned(),
            },
        }
    }
}

impl From<CatalogTextRef<'_>> for StructuredTextData {
    fn from(text: CatalogTextRef<'_>) -> Self {
        Self::Catalog {
            code: text.code().to_owned(),
            args: text.iter_args().map(CatalogArgData::from).collect(),
        }
    }
}

impl From<structured_text_kit::CatalogArgRef<'_>> for CatalogArgData {
    fn from(arg: structured_text_kit::CatalogArgRef<'_>) -> Self {
        Self {
            name: arg.name().to_owned(),
            value: CatalogArgValueData::from(arg.value()),
        }
    }
}

impl From<CatalogArgValueRef<'_>> for CatalogArgValueData {
    fn from(value: CatalogArgValueRef<'_>) -> Self {
        match value {
            CatalogArgValueRef::Text(value) => Self::Text(value.to_owned()),
            CatalogArgValueRef::Bool(value) => Self::Bool(value),
            CatalogArgValueRef::Signed(value) => Self::Signed(value.to_string()),
            CatalogArgValueRef::Unsigned(value) => Self::Unsigned(value.to_string()),
            CatalogArgValueRef::NestedText(text) => {
                Self::NestedText(Box::new(StructuredTextData::from(text)))
            }
            _ => unreachable!("unsupported non-exhaustive CatalogArgValueRef variant"),
        }
    }
}

impl TryFrom<&StructuredTextData> for StructuredText {
    type Error = StructuredTextDataError;

    fn try_from(data: &StructuredTextData) -> Result<Self, Self::Error> {
        match data {
            StructuredTextData::Catalog { code, args } => {
                let mut text = CatalogText::try_new(code.clone())?;
                for arg in args {
                    match &arg.value {
                        CatalogArgValueData::Text(value) => {
                            text.try_with_value_arg(arg.name.clone(), value.clone())?;
                        }
                        CatalogArgValueData::Bool(value) => {
                            text.try_with_value_arg(arg.name.clone(), *value)?;
                        }
                        CatalogArgValueData::Signed(value) => {
                            let parsed = value.parse::<i128>().map_err(|_| {
                                StructuredTextDataError::InvalidSignedInteger {
                                    arg_name: arg.name.clone(),
                                    value: value.clone(),
                                }
                            })?;
                            text.try_with_value_arg(arg.name.clone(), parsed)?;
                        }
                        CatalogArgValueData::Unsigned(value) => {
                            let parsed = value.parse::<u128>().map_err(|_| {
                                StructuredTextDataError::InvalidUnsignedInteger {
                                    arg_name: arg.name.clone(),
                                    value: value.clone(),
                                }
                            })?;
                            text.try_with_value_arg(arg.name.clone(), parsed)?;
                        }
                        CatalogArgValueData::NestedText(value) => {
                            text.try_with_nested_text_arg(
                                arg.name.clone(),
                                StructuredText::try_from(value.as_ref())?,
                            )?;
                        }
                    }
                }
                Ok(StructuredText::from(text))
            }
            StructuredTextData::Freeform { text } => Ok(StructuredText::freeform(text.clone())),
        }
    }
}

impl TryFrom<StructuredTextData> for StructuredText {
    type Error = StructuredTextDataError;

    fn try_from(data: StructuredTextData) -> Result<Self, Self::Error> {
        StructuredText::try_from(&data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_round_trip_preserves_typed_args_and_nesting() {
        let mut nested =
            CatalogText::try_new("execpolicy_load_denied.child").expect("valid nested code");
        nested
            .try_with_value_arg("source", "mode")
            .expect("nested arg should validate");

        let mut text = CatalogText::try_new("execpolicy_load_denied").expect("valid code");
        text.try_with_value_arg("mode", "danger")
            .expect("mode arg should validate");
        text.try_with_value_arg("retryable", false)
            .expect("bool arg should validate");
        text.try_with_value_arg("attempt", 12_i64)
            .expect("signed arg should validate");
        text.try_with_value_arg("bytes", 42_u64)
            .expect("unsigned arg should validate");
        text.try_with_nested_text_arg("cause", StructuredText::from(nested))
            .expect("nested arg should validate");

        let original = StructuredText::from(text);
        let wire = StructuredTextData::from(&original);
        let round_trip = StructuredText::try_from(wire).expect("wire payload should deserialize");
        assert_eq!(round_trip, original);
    }

    #[test]
    fn freeform_round_trip_preserves_text() {
        let original = StructuredText::freeform("plain text");
        let wire = StructuredTextData::from(&original);
        assert_eq!(wire.freeform_text(), Some("plain text"));
        let round_trip = StructuredText::try_from(wire).expect("freeform payload should parse");
        assert_eq!(round_trip, original);
    }

    #[test]
    fn invalid_catalog_code_is_rejected() {
        let err = StructuredText::try_from(StructuredTextData::Catalog {
            code: "bad code".to_string(),
            args: Vec::new(),
        })
        .expect_err("invalid code should be rejected");
        assert!(matches!(
            err,
            StructuredTextDataError::Validation(StructuredTextValidationError::InvalidCode(_))
        ));
    }

    #[test]
    fn invalid_signed_integer_is_rejected() {
        let err = StructuredText::try_from(StructuredTextData::Catalog {
            code: "mode_denied".to_string(),
            args: vec![CatalogArgData {
                name: "attempt".to_string(),
                value: CatalogArgValueData::Signed("not-an-int".to_string()),
            }],
        })
        .expect_err("invalid integer should be rejected");
        assert!(matches!(
            err,
            StructuredTextDataError::InvalidSignedInteger { .. }
        ));
    }

    #[test]
    fn catalog_code_reader_only_reports_catalog_texts() {
        assert_eq!(
            StructuredTextData::Catalog {
                code: "mode_denied".to_string(),
                args: Vec::new(),
            }
            .catalog_code(),
            Some("mode_denied")
        );
        assert_eq!(
            StructuredTextData::Freeform {
                text: "plain".to_string(),
            }
            .catalog_code(),
            None
        );
    }
}
