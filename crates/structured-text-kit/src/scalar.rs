use crate::text::ArgValue;

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StructuredTextScalarValue {
    Text(String),
    Bool(bool),
    Signed(i128),
    Unsigned(u128),
}

mod private {
    pub trait Sealed {}
}

#[diagnostic::on_unimplemented(
    message = "unsupported structured-text scalar argument type",
    label = "this value is not a supported scalar arg; use a nested structured text for non-scalar values",
    note = "supported scalar args are text, booleans, and integer types"
)]
pub trait StructuredTextScalarArg: private::Sealed {
    #[doc(hidden)]
    fn into_scalar_value(self) -> StructuredTextScalarValue;
}

impl From<StructuredTextScalarValue> for ArgValue {
    fn from(value: StructuredTextScalarValue) -> Self {
        match value {
            StructuredTextScalarValue::Text(value) => Self::Text(value),
            StructuredTextScalarValue::Bool(value) => Self::Bool(value),
            StructuredTextScalarValue::Signed(value) => Self::Signed(value),
            StructuredTextScalarValue::Unsigned(value) => Self::Unsigned(value),
        }
    }
}

impl private::Sealed for StructuredTextScalarValue {}

impl StructuredTextScalarArg for StructuredTextScalarValue {
    fn into_scalar_value(self) -> StructuredTextScalarValue {
        self
    }
}

macro_rules! impl_from_signed_int {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl private::Sealed for $ty {}

            impl StructuredTextScalarArg for $ty {
                fn into_scalar_value(self) -> StructuredTextScalarValue {
                    StructuredTextScalarValue::from(self)
                }
            }

            impl From<$ty> for StructuredTextScalarValue {
                fn from(value: $ty) -> Self {
                    Self::Signed(i128::from(value))
                }
            }
        )+
    };
}

macro_rules! impl_from_unsigned_int {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl private::Sealed for $ty {}

            impl StructuredTextScalarArg for $ty {
                fn into_scalar_value(self) -> StructuredTextScalarValue {
                    StructuredTextScalarValue::from(self)
                }
            }

            impl From<$ty> for StructuredTextScalarValue {
                fn from(value: $ty) -> Self {
                    Self::Unsigned(u128::from(value))
                }
            }
        )+
    };
}

impl private::Sealed for isize {}

impl StructuredTextScalarArg for isize {
    fn into_scalar_value(self) -> StructuredTextScalarValue {
        StructuredTextScalarValue::from(self)
    }
}

impl From<isize> for StructuredTextScalarValue {
    fn from(value: isize) -> Self {
        Self::Signed(value as i128)
    }
}

impl private::Sealed for usize {}

impl StructuredTextScalarArg for usize {
    fn into_scalar_value(self) -> StructuredTextScalarValue {
        StructuredTextScalarValue::from(self)
    }
}

impl From<usize> for StructuredTextScalarValue {
    fn from(value: usize) -> Self {
        Self::Unsigned(value as u128)
    }
}

impl private::Sealed for String {}

impl StructuredTextScalarArg for String {
    fn into_scalar_value(self) -> StructuredTextScalarValue {
        StructuredTextScalarValue::from(self)
    }
}

impl From<String> for StructuredTextScalarValue {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl private::Sealed for &str {}

impl StructuredTextScalarArg for &str {
    fn into_scalar_value(self) -> StructuredTextScalarValue {
        StructuredTextScalarValue::from(self)
    }
}

impl From<&str> for StructuredTextScalarValue {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl private::Sealed for &String {}

impl StructuredTextScalarArg for &String {
    fn into_scalar_value(self) -> StructuredTextScalarValue {
        StructuredTextScalarValue::from(self)
    }
}

impl From<&String> for StructuredTextScalarValue {
    fn from(value: &String) -> Self {
        Self::Text(value.clone())
    }
}

impl private::Sealed for bool {}

impl StructuredTextScalarArg for bool {
    fn into_scalar_value(self) -> StructuredTextScalarValue {
        StructuredTextScalarValue::from(self)
    }
}

impl From<bool> for StructuredTextScalarValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl_from_signed_int!(i8, i16, i32, i64, i128);
impl_from_unsigned_int!(u8, u16, u32, u64, u128);
