#[macro_export]
macro_rules! structured_text {
    (@__validate $value:literal, $message:literal) => {
        const _: () = {
            if !$crate::__structured_text_component_is_valid($value) {
                panic!($message);
            }
        };
    };
    (@__assert_distinct $lhs:literal, $rhs:literal, $message:literal) => {
        const _: () = {
            if $crate::__structured_text_literals_equal($lhs, $rhs) {
                panic!($message);
            }
        };
    };
    (@__check_unique_names [$($seen:literal),*]) => {};
    (@__check_unique_names [$($seen:literal),*],) => {};
    (@__check_unique_names [$($seen:literal),*], $name:literal => $value:expr $(, $($tail:tt)*)?) => {{
        $(
            $crate::structured_text!(
                @__assert_distinct
                $seen,
                $name,
                "structured text arg names must be unique"
            );
        )*
        $crate::structured_text!(@__check_unique_names [$($seen,)* $name] $(, $($tail)*)?);
    }};
    (@__check_unique_names [$($seen:literal),*], $name:expr => @text $value:expr $(, $($tail:tt)*)?) => {};
    (@__check_unique_names [$($seen:literal),*], $name:expr => $value:expr $(, $($tail:tt)*)?) => {};
    (@__accum $text:expr) => {
        $crate::StructuredText::from($text)
    };
    (@__accum $text:expr,) => {
        $crate::StructuredText::from($text)
    };
    (@__accum $text:expr, $name:literal => @text $value:expr $(, $($tail:tt)*)?) => {
        ::core::compile_error!(
            "structured_text! does not accept nested structured texts; use try_structured_text! for @text args"
        )
    };
    (@__accum $text:expr, $name:literal => $value:expr $(, $($tail:tt)*)?) => {{
        $crate::structured_text!(
            @__validate
            $name,
            "structured text arg name must contain non-empty ASCII [A-Za-z0-9_-] segments separated by '.'"
        );
        let mut text = $text;
        text
            .try_with_value_arg($name, $value)
            .expect("literal structured-text scalar arguments must remain valid");
        $crate::structured_text!(
            @__accum
            text
            $(, $($tail)*)?
        )
    }};
    (@__accum $text:expr, $name:expr => @text $value:expr $(, $($tail:tt)*)?) => {
        ::core::compile_error!(
            "structured_text! does not accept nested structured texts; use try_structured_text! for @text args"
        )
    };
    (@__accum $text:expr, $name:expr => $value:expr $(, $($tail:tt)*)?) => {
        ::core::compile_error!(
            "structured_text! requires string literal arg names; use try_structured_text! for runtime arg names"
        )
    };
    ($code:literal $(, $($rest:tt)*)?) => {{
        $crate::structured_text!(
            @__validate
            $code,
            "structured text code must contain non-empty ASCII [A-Za-z0-9_-] segments separated by '.'"
        );
        $crate::structured_text!(
            @__check_unique_names
            []
            $(, $($rest)*)?
        );
        $crate::structured_text!(
            @__accum
            $crate::CatalogText::try_new($code)
                .expect("literal structured-text codes must remain valid")
            $(, $($rest)*)?
        )
    }};
    ($code:expr $(,)?) => {
        ::core::compile_error!(
            "structured_text! requires a string literal catalog code; use try_structured_text! for runtime codes"
        )
    };
    ($code:expr, $($rest:tt)*) => {
        ::core::compile_error!(
            "structured_text! requires a string literal catalog code; use try_structured_text! for runtime codes"
        )
    };
}

#[macro_export]
macro_rules! try_structured_text {
    (@__validate $value:literal, $message:literal) => {
        const _: () = {
            if !$crate::__structured_text_component_is_valid($value) {
                panic!($message);
            }
        };
    };
    (@__accum $text:expr) => {
        $text.map($crate::StructuredText::from)
    };
    (@__accum $text:expr,) => {
        $text.map($crate::StructuredText::from)
    };
    (@__accum $text:expr, $name:literal => @text $value:expr $(, $($tail:tt)*)?) => {{
        $crate::try_structured_text!(
            @__validate
            $name,
            "structured text arg name must contain non-empty ASCII [A-Za-z0-9_-] segments separated by '.'"
        );
        $crate::try_structured_text!(
            @__accum
            $text.and_then(|mut text| {
                text.try_with_nested_text_arg($name, $value)?;
                Ok(text)
            })
            $(, $($tail)*)?
        )
    }};
    (@__accum $text:expr, $name:expr => @text $value:expr $(, $($tail:tt)*)?) => {
        $crate::try_structured_text!(
            @__accum
            $text.and_then(|mut text| {
                text.try_with_nested_text_arg($name, $value)?;
                Ok(text)
            })
            $(, $($tail)*)?
        )
    };
    (@__accum $text:expr, $name:literal => $value:expr $(, $($tail:tt)*)?) => {{
        $crate::try_structured_text!(
            @__validate
            $name,
            "structured text arg name must contain non-empty ASCII [A-Za-z0-9_-] segments separated by '.'"
        );
        $crate::try_structured_text!(
            @__accum
            $text.and_then(|mut text| {
                text.try_with_value_arg($name, $value)?;
                Ok(text)
            })
            $(, $($tail)*)?
        )
    }};
    (@__accum $text:expr, $name:expr => $value:expr $(, $($tail:tt)*)?) => {
        $crate::try_structured_text!(
            @__accum
            $text.and_then(|mut text| {
                text.try_with_value_arg($name, $value)?;
                Ok(text)
            })
            $(, $($tail)*)?
        )
    };
    ($code:literal $(, $($rest:tt)*)?) => {{
        $crate::try_structured_text!(
            @__validate
            $code,
            "structured text code must contain non-empty ASCII [A-Za-z0-9_-] segments separated by '.'"
        );
        $crate::try_structured_text!(
            @__accum
            $crate::CatalogText::try_new($code)
            $(, $($rest)*)?
        )
    }};
    ($code:expr $(,)?) => {
        $crate::CatalogText::try_new($code).map($crate::StructuredText::from)
    };
    ($code:expr, $($rest:tt)*) => {
        $crate::try_structured_text!(@__accum $crate::CatalogText::try_new($code), $($rest)*)
    };
}
