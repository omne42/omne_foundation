use crate::scalar::StructuredTextScalarArg;
use crate::validation::{
    MAX_TEXT_DEPTH, StructuredTextValidationError, validate_nested_text, validate_text_arg_name,
    validate_text_code,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredText {
    pub(crate) kind: StructuredTextKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogText {
    pub(crate) code: String,
    pub(crate) args: Vec<StructuredTextArg>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogTextRef<'a> {
    pub(crate) code: &'a str,
    pub(crate) args: &'a [StructuredTextArg],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogArgRef<'a> {
    pub(crate) name: &'a str,
    pub(crate) value: &'a ArgValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructuredTextRef<'a> {
    Catalog(CatalogTextRef<'a>),
    Freeform(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CatalogArgValueRef<'a> {
    Text(&'a str),
    Bool(bool),
    Signed(i128),
    Unsigned(u128),
    NestedText(StructuredTextRef<'a>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StructuredTextKind {
    Catalog(CatalogText),
    Freeform { text: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StructuredTextArg {
    pub(crate) name: String,
    pub(crate) value: ArgValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArgValue {
    Text(String),
    Bool(bool),
    Signed(i128),
    Unsigned(u128),
    NestedText(Box<StructuredText>),
}

impl<'a> StructuredTextRef<'a> {
    #[must_use]
    pub fn as_catalog(self) -> Option<CatalogTextRef<'a>> {
        match self {
            Self::Catalog(text) => Some(text),
            Self::Freeform(_) => None,
        }
    }

    #[must_use]
    pub fn freeform_text(self) -> Option<&'a str> {
        match self {
            Self::Catalog(_) => None,
            Self::Freeform(text) => Some(text),
        }
    }
}

impl<'a> CatalogTextRef<'a> {
    #[must_use]
    pub fn code(self) -> &'a str {
        self.code
    }

    #[must_use]
    pub fn iter_args(self) -> impl ExactSizeIterator<Item = CatalogArgRef<'a>> + 'a {
        self.args.iter().map(CatalogArgRef::from_internal)
    }

    #[must_use]
    pub fn arg(self, name: &str) -> Option<CatalogArgRef<'a>> {
        find_arg_index(self.args, name)
            .ok()
            .map(|index| CatalogArgRef::from_internal(&self.args[index]))
    }

    #[must_use]
    pub fn text_arg(self, name: &str) -> Option<&'a str> {
        self.arg(name).and_then(CatalogArgRef::text)
    }

    #[must_use]
    pub fn bool_arg(self, name: &str) -> Option<bool> {
        self.arg(name).and_then(CatalogArgRef::bool_value)
    }

    #[must_use]
    pub fn signed_arg(self, name: &str) -> Option<i128> {
        self.arg(name).and_then(CatalogArgRef::signed_value)
    }

    #[must_use]
    pub fn unsigned_arg(self, name: &str) -> Option<u128> {
        self.arg(name).and_then(CatalogArgRef::unsigned_value)
    }

    #[must_use]
    pub fn nested_text_arg(self, name: &str) -> Option<StructuredTextRef<'a>> {
        self.arg(name).and_then(CatalogArgRef::nested_text_value)
    }
}

impl<'a> CatalogArgRef<'a> {
    pub(crate) fn from_internal(arg: &'a StructuredTextArg) -> Self {
        Self {
            name: arg.name.as_str(),
            value: &arg.value,
        }
    }

    #[must_use]
    pub fn value(self) -> CatalogArgValueRef<'a> {
        CatalogArgValueRef::from_internal(self.value)
    }

    #[must_use]
    pub fn name(self) -> &'a str {
        self.name
    }

    #[must_use]
    pub fn text(self) -> Option<&'a str> {
        match self.value() {
            CatalogArgValueRef::Text(value) => Some(value),
            CatalogArgValueRef::Bool(_)
            | CatalogArgValueRef::Signed(_)
            | CatalogArgValueRef::Unsigned(_)
            | CatalogArgValueRef::NestedText(_) => None,
        }
    }

    #[must_use]
    pub fn bool_value(self) -> Option<bool> {
        match self.value() {
            CatalogArgValueRef::Bool(value) => Some(value),
            CatalogArgValueRef::Text(_)
            | CatalogArgValueRef::Signed(_)
            | CatalogArgValueRef::Unsigned(_)
            | CatalogArgValueRef::NestedText(_) => None,
        }
    }

    #[must_use]
    pub fn signed_value(self) -> Option<i128> {
        match self.value() {
            CatalogArgValueRef::Signed(value) => Some(value),
            CatalogArgValueRef::Text(_)
            | CatalogArgValueRef::Bool(_)
            | CatalogArgValueRef::Unsigned(_)
            | CatalogArgValueRef::NestedText(_) => None,
        }
    }

    #[must_use]
    pub fn unsigned_value(self) -> Option<u128> {
        match self.value() {
            CatalogArgValueRef::Unsigned(value) => Some(value),
            CatalogArgValueRef::Text(_)
            | CatalogArgValueRef::Bool(_)
            | CatalogArgValueRef::Signed(_)
            | CatalogArgValueRef::NestedText(_) => None,
        }
    }

    #[must_use]
    pub fn nested_text_value(self) -> Option<StructuredTextRef<'a>> {
        match self.value() {
            CatalogArgValueRef::NestedText(text) => Some(text),
            CatalogArgValueRef::Text(_)
            | CatalogArgValueRef::Bool(_)
            | CatalogArgValueRef::Signed(_)
            | CatalogArgValueRef::Unsigned(_) => None,
        }
    }
}

impl<'a> CatalogArgValueRef<'a> {
    pub(crate) fn from_internal(value: &'a ArgValue) -> Self {
        match value {
            ArgValue::Text(value) => Self::Text(value.as_str()),
            ArgValue::Bool(value) => Self::Bool(*value),
            ArgValue::Signed(value) => Self::Signed(*value),
            ArgValue::Unsigned(value) => Self::Unsigned(*value),
            ArgValue::NestedText(text) => Self::NestedText(text.text_ref()),
        }
    }
}

impl StructuredTextArg {
    #[must_use]
    pub(crate) fn name(&self) -> &str {
        self.name.as_str()
    }

    #[must_use]
    pub(crate) fn nested_text_value(&self) -> Option<&StructuredText> {
        match &self.value {
            ArgValue::NestedText(text) => Some(text.as_ref()),
            ArgValue::Text(_) | ArgValue::Bool(_) | ArgValue::Signed(_) | ArgValue::Unsigned(_) => {
                None
            }
        }
    }
}

pub(crate) fn find_arg_index(args: &[StructuredTextArg], name: &str) -> Result<usize, usize> {
    args.binary_search_by(|existing| existing.name().cmp(name))
}

impl CatalogText {
    pub fn try_new(code: impl Into<String>) -> Result<Self, StructuredTextValidationError> {
        Ok(Self {
            code: validate_text_code(code)?,
            args: Vec::new(),
        })
    }

    #[must_use]
    pub fn code(&self) -> &str {
        self.code.as_str()
    }

    #[must_use]
    pub fn as_ref(&self) -> CatalogTextRef<'_> {
        CatalogTextRef {
            code: self.code(),
            args: self.args.as_slice(),
        }
    }

    pub fn try_with_value_arg<V>(
        &mut self,
        name: impl Into<String>,
        value: V,
    ) -> Result<(), StructuredTextValidationError>
    where
        V: StructuredTextScalarArg,
    {
        let arg = StructuredTextArg {
            name: validate_text_arg_name(name)?,
            value: value.into_scalar_value().into(),
        };
        try_insert_arg(&mut self.args, arg)?;
        Ok(())
    }

    pub fn try_with_nested_text_arg(
        &mut self,
        name: impl Into<String>,
        text: impl Into<StructuredText>,
    ) -> Result<(), StructuredTextValidationError> {
        let arg = StructuredTextArg::try_nested_text(name, text)?;
        try_insert_arg(&mut self.args, arg)?;
        Ok(())
    }
}

impl StructuredText {
    fn freeform_from_text(text: impl Into<String>) -> Self {
        Self {
            kind: StructuredTextKind::Freeform { text: text.into() },
        }
    }

    #[must_use]
    pub fn freeform(text: impl Into<String>) -> Self {
        Self::freeform_from_text(text)
    }

    #[must_use]
    pub fn text_ref(&self) -> StructuredTextRef<'_> {
        match &self.kind {
            StructuredTextKind::Catalog(text) => StructuredTextRef::Catalog(text.as_ref()),
            StructuredTextKind::Freeform { text } => StructuredTextRef::Freeform(text),
        }
    }

    #[must_use]
    pub fn as_catalog(&self) -> Option<CatalogTextRef<'_>> {
        match self.text_ref() {
            StructuredTextRef::Catalog(text) => Some(text),
            StructuredTextRef::Freeform(_) => None,
        }
    }

    #[must_use]
    pub fn freeform_text(&self) -> Option<&str> {
        match self.text_ref() {
            StructuredTextRef::Catalog(_) => None,
            StructuredTextRef::Freeform(text) => Some(text),
        }
    }

    pub(crate) fn exceeds_max_depth(&self, current_depth: usize) -> bool {
        if current_depth > MAX_TEXT_DEPTH {
            return true;
        }

        match &self.kind {
            StructuredTextKind::Catalog(text) => text
                .args
                .iter()
                .filter_map(StructuredTextArg::nested_text_value)
                .any(|text| text.exceeds_max_depth(current_depth + 1)),
            StructuredTextKind::Freeform { .. } => false,
        }
    }
}

impl StructuredTextArg {
    pub(crate) fn try_nested_text(
        name: impl Into<String>,
        text: impl Into<StructuredText>,
    ) -> Result<Self, StructuredTextValidationError> {
        let name = validate_text_arg_name(name)?;
        let text = text.into();
        validate_nested_text(&text)?;
        Ok(Self {
            name,
            value: ArgValue::NestedText(Box::new(text)),
        })
    }
}

impl From<CatalogText> for StructuredText {
    fn from(text: CatalogText) -> Self {
        Self {
            kind: StructuredTextKind::Catalog(text),
        }
    }
}

fn try_insert_arg(
    args: &mut Vec<StructuredTextArg>,
    arg: StructuredTextArg,
) -> Result<(), StructuredTextValidationError> {
    let duplicate_name = arg.name.clone();

    match find_arg_index(args, arg.name()) {
        Ok(_) => Err(StructuredTextValidationError::DuplicateArgName(
            duplicate_name,
        )),
        Err(index) => {
            args.insert(index, arg);
            Ok(())
        }
    }
}
