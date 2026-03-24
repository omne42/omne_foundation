use crate::text::{
    CatalogArgRef, CatalogArgValueRef, CatalogText, CatalogTextRef, StructuredText,
    StructuredTextArg, StructuredTextRef,
};
use serde::ser::{SerializeSeq, SerializeStruct};
use serde::{Serialize, Serializer};

impl Serialize for StructuredText {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.text_ref().serialize(serializer)
    }
}

impl Serialize for CatalogText {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_ref().serialize(serializer)
    }
}

impl Serialize for StructuredTextRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match *self {
            Self::Catalog(text) => {
                let mut state = serializer.serialize_struct("StructuredText", 3)?;
                state.serialize_field("kind", "catalog")?;
                state.serialize_field("code", &text.code())?;
                state.serialize_field("args", &CatalogArgs(text.args))?;
                state.end()
            }
            Self::Freeform(text) => {
                let mut state = serializer.serialize_struct("StructuredText", 2)?;
                state.serialize_field("kind", "freeform")?;
                state.serialize_field("text", text)?;
                state.end()
            }
        }
    }
}

impl Serialize for CatalogTextRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_catalog_text(self.code, self.args, serializer)
    }
}

impl Serialize for CatalogArgRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("CatalogArg", 2)?;
        state.serialize_field("name", self.name)?;
        state.serialize_field("value", &self.value())?;
        state.end()
    }
}

impl Serialize for CatalogArgValueRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match *self {
            Self::Text(value) => serialize_typed_value("text", value, serializer),
            Self::Bool(value) => serialize_typed_value("bool", value, serializer),
            Self::Signed(value) => serialize_typed_value("signed", value.to_string(), serializer),
            Self::Unsigned(value) => {
                serialize_typed_value("unsigned", value.to_string(), serializer)
            }
            Self::NestedText(text) => serialize_typed_value("nested_text", text, serializer),
        }
    }
}

struct CatalogArgs<'a>(&'a [StructuredTextArg]);

impl Serialize for CatalogArgs<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for arg in self.0 {
            let arg_ref = CatalogArgRef::from_internal(arg);
            seq.serialize_element(&arg_ref)?;
        }
        seq.end()
    }
}

fn serialize_catalog_text<S: Serializer>(
    code: &str,
    args: &[StructuredTextArg],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut state = serializer.serialize_struct("CatalogText", 2)?;
    state.serialize_field("code", code)?;
    state.serialize_field("args", &CatalogArgs(args))?;
    state.end()
}

fn serialize_typed_value<S: Serializer, V: Serialize>(
    kind: &str,
    value: V,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut state = serializer.serialize_struct("CatalogArgValue", 2)?;
    state.serialize_field("kind", kind)?;
    state.serialize_field("value", &value)?;
    state.end()
}
