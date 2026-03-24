use crate::text::{
    CatalogArgValueRef, CatalogText, StructuredText, StructuredTextArg, StructuredTextRef,
};
use crate::validation::{
    DUPLICATE_TEXT_ARG_NAME, INVALID_TEXT_ARG_NAME, INVALID_TEXT_CODE,
    StructuredTextValidationError,
};
use std::error::Error as StdError;
use std::fmt::{self, Display, Formatter};

impl StructuredText {
    #[must_use]
    pub fn diagnostic_display(&self) -> impl Display + '_ {
        DiagnosticDisplay(self)
    }
}

impl<'a> StructuredTextRef<'a> {
    #[must_use]
    pub fn diagnostic_display(self) -> impl Display + 'a {
        DiagnosticDisplayRef(self)
    }
}

impl Display for CatalogText {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        fmt_catalog_text(self.code(), self.args.as_slice(), f)
    }
}

impl Display for StructuredText {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        fmt_display_text(self, f)
    }
}

impl Display for StructuredTextArg {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        fmt_arg(self, f)
    }
}

impl Display for StructuredTextValidationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCode(value) => write!(f, "{INVALID_TEXT_CODE}: {value:?}"),
            Self::InvalidArgName(value) => {
                write!(f, "{INVALID_TEXT_ARG_NAME}: {value:?}")
            }
            Self::DuplicateArgName(value) => {
                write!(f, "{DUPLICATE_TEXT_ARG_NAME}: {value:?}")
            }
            Self::NestingTooDeep { max_depth } => {
                write!(f, "structured texts may nest at most {max_depth} levels")
            }
        }
    }
}

impl StdError for StructuredTextValidationError {}

impl Display for CatalogArgValueRef<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        fmt_catalog_arg_value_ref(*self, f)
    }
}

struct DiagnosticDisplay<'a>(&'a StructuredText);
struct DiagnosticDisplayRef<'a>(StructuredTextRef<'a>);

fn fmt_catalog_text(code: &str, args: &[StructuredTextArg], f: &mut Formatter<'_>) -> fmt::Result {
    f.write_str(code)?;
    if args.is_empty() {
        return Ok(());
    }

    f.write_str(" {")?;
    for (index, arg) in args.iter().enumerate() {
        if index > 0 {
            f.write_str(", ")?;
        }
        fmt_arg(arg, f)?;
    }
    f.write_str("}")
}

fn fmt_arg(arg: &StructuredTextArg, f: &mut Formatter<'_>) -> fmt::Result {
    f.write_str(arg.name())?;
    f.write_str("=")?;
    fmt_catalog_value_ref(CatalogArgValueRef::from_internal(&arg.value), f)
}

fn fmt_catalog_arg_value_ref(value: CatalogArgValueRef<'_>, f: &mut Formatter<'_>) -> fmt::Result {
    fmt_catalog_value_ref(value, f)
}

fn fmt_catalog_value_ref(value: CatalogArgValueRef<'_>, f: &mut Formatter<'_>) -> fmt::Result {
    match value {
        CatalogArgValueRef::Text(value) => write!(f, "{value:?}"),
        CatalogArgValueRef::Bool(value) => Display::fmt(&value, f),
        CatalogArgValueRef::Signed(value) => Display::fmt(&value, f),
        CatalogArgValueRef::Unsigned(value) => Display::fmt(&value, f),
        CatalogArgValueRef::NestedText(text) => fmt_diagnostic_text_ref(text, f),
    }
}

fn fmt_freeform_text(text: &str, f: &mut Formatter<'_>) -> fmt::Result {
    f.write_str("@freeform(")?;
    write!(f, "{text:?}")?;
    f.write_str(")")
}

fn fmt_diagnostic_text(text: &StructuredText, f: &mut Formatter<'_>) -> fmt::Result {
    fmt_diagnostic_text_ref(text.text_ref(), f)
}

fn fmt_diagnostic_text_ref(text: StructuredTextRef<'_>, f: &mut Formatter<'_>) -> fmt::Result {
    match text {
        StructuredTextRef::Catalog(text) => fmt_catalog_text(text.code(), text.args, f),
        StructuredTextRef::Freeform(text) => fmt_freeform_text(text, f),
    }
}

fn fmt_display_text(text: &StructuredText, f: &mut Formatter<'_>) -> fmt::Result {
    match text.text_ref() {
        StructuredTextRef::Catalog(text) => fmt_catalog_text(text.code(), text.args, f),
        StructuredTextRef::Freeform(text) => f.write_str(text),
    }
}

impl Display for DiagnosticDisplay<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        fmt_diagnostic_text(self.0, f)
    }
}

impl Display for DiagnosticDisplayRef<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        fmt_diagnostic_text_ref(self.0, f)
    }
}
