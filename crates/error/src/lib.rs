use std::fmt::{self, Display, Formatter};

use i18n::{Locale, MessageArg, MessageCatalog, render_message};

fn msg_arg<'a>(name: &'static str, value: impl Into<std::borrow::Cow<'a, str>>) -> MessageArg<'a> {
    MessageArg::new(name, value)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredMessage {
    code: &'static str,
    args: Vec<StructuredMessageArg>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredMessageArg {
    name: &'static str,
    value: StructuredMessageValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructuredMessageValue {
    Text(String),
    Message(Box<StructuredMessage>),
}

impl StructuredMessage {
    #[must_use]
    pub fn new(code: &'static str) -> Self {
        Self {
            code,
            args: Vec::new(),
        }
    }

    #[must_use]
    pub fn freeform(message: impl ToString) -> Self {
        Self::new("error_detail.freeform").arg("message", message)
    }

    #[must_use]
    pub fn arg(mut self, name: &'static str, value: impl ToString) -> Self {
        self.args.push(StructuredMessageArg::new(name, value));
        self
    }

    #[must_use]
    pub fn arg_message(mut self, name: &'static str, message: StructuredMessage) -> Self {
        self.args.push(StructuredMessageArg::message(name, message));
        self
    }

    #[must_use]
    pub fn code(&self) -> &'static str {
        self.code
    }

    #[must_use]
    pub fn args(&self) -> &[StructuredMessageArg] {
        &self.args
    }

    #[must_use]
    pub fn render_with_catalog<C>(&self, catalog: &C, locale: Locale) -> String
    where
        C: MessageCatalog + ?Sized,
    {
        if let Some(message) = self.freeform_message() {
            return message.to_string();
        }

        let args = self
            .args()
            .iter()
            .map(|arg| {
                let value = if let Some(value) = arg.text() {
                    std::borrow::Cow::Borrowed(value)
                } else if let Some(message) = arg.message_value() {
                    std::borrow::Cow::Owned(message.render_with_catalog(catalog, locale))
                } else {
                    std::borrow::Cow::Borrowed("")
                };
                msg_arg(arg.name(), value)
            })
            .collect::<Vec<_>>();
        render_message(catalog, locale, self.code(), &args)
    }

    fn freeform_message(&self) -> Option<&str> {
        (self.code == "error_detail.freeform")
            .then(|| self.args.iter().find(|arg| arg.name == "message"))
            .flatten()
            .and_then(StructuredMessageArg::text)
    }
}

impl StructuredMessageArg {
    #[must_use]
    pub fn new(name: &'static str, value: impl ToString) -> Self {
        Self {
            name,
            value: StructuredMessageValue::Text(value.to_string()),
        }
    }

    #[must_use]
    pub fn message(name: &'static str, message: StructuredMessage) -> Self {
        Self {
            name,
            value: StructuredMessageValue::Message(Box::new(message)),
        }
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    #[must_use]
    pub fn text(&self) -> Option<&str> {
        match &self.value {
            StructuredMessageValue::Text(value) => Some(value.as_str()),
            StructuredMessageValue::Message(_) => None,
        }
    }

    #[must_use]
    pub fn message_value(&self) -> Option<&StructuredMessage> {
        match &self.value {
            StructuredMessageValue::Text(_) => None,
            StructuredMessageValue::Message(message) => Some(message.as_ref()),
        }
    }
}

impl Display for StructuredMessage {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if let Some(message) = self.freeform_message() {
            return f.write_str(message);
        }

        f.write_str(self.code)?;
        if self.args.is_empty() {
            return Ok(());
        }

        f.write_str(" (")?;
        for (index, arg) in self.args.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            f.write_str(arg.name())?;
            f.write_str("=")?;
            if let Some(value) = arg.text() {
                f.write_str(value)?;
            } else if let Some(message) = arg.message_value() {
                Display::fmt(message, f)?;
            }
        }
        f.write_str(")")
    }
}

#[must_use]
pub fn render_structured_message<C>(
    catalog: &C,
    locale: Locale,
    message: &StructuredMessage,
) -> String
where
    C: MessageCatalog + ?Sized,
{
    message.render_with_catalog(catalog, locale)
}

pub trait LocalizedMessage {
    fn render_localized<C>(&self, catalog: &C, locale: Locale) -> String
    where
        C: MessageCatalog + ?Sized;
}

#[must_use]
pub fn render_localized<T, C>(value: &T, catalog: &C, locale: Locale) -> String
where
    T: LocalizedMessage + ?Sized,
    C: MessageCatalog + ?Sized,
{
    value.render_localized(catalog, locale)
}

pub struct LocalizedDisplay<'a, T, C>
where
    T: LocalizedMessage + ?Sized,
    C: MessageCatalog + ?Sized,
{
    value: &'a T,
    catalog: &'a C,
    locale: Locale,
}

impl<'a, T, C> LocalizedDisplay<'a, T, C>
where
    T: LocalizedMessage + ?Sized,
    C: MessageCatalog + ?Sized,
{
    #[must_use]
    pub fn new(value: &'a T, catalog: &'a C, locale: Locale) -> Self {
        Self {
            value,
            catalog,
            locale,
        }
    }
}

impl<T, C> Display for LocalizedDisplay<'_, T, C>
where
    T: LocalizedMessage + ?Sized,
    C: MessageCatalog + ?Sized,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.value.render_localized(self.catalog, self.locale))
    }
}

#[must_use]
pub fn localized<'a, T, C>(
    value: &'a T,
    catalog: &'a C,
    locale: Locale,
) -> LocalizedDisplay<'a, T, C>
where
    T: LocalizedMessage + ?Sized,
    C: MessageCatalog + ?Sized,
{
    LocalizedDisplay::new(value, catalog, locale)
}

#[macro_export]
#[doc(hidden)]
macro_rules! __structured_message_chain {
    ($message:expr $(,)?) => {
        $message
    };
    ($message:expr, $name:literal => @message $value:expr $(, $($rest:tt)*)?) => {
        $crate::__structured_message_chain!(
            $message.arg_message($name, $value)
            $(, $($rest)*)?
        )
    };
    ($message:expr, $name:literal => $value:expr $(, $($rest:tt)*)?) => {
        $crate::__structured_message_chain!(
            $message.arg($name, $value)
            $(, $($rest)*)?
        )
    };
}

#[macro_export]
macro_rules! structured_message {
    ($code:expr $(,)?) => {
        $crate::StructuredMessage::new($code)
    };
    ($code:expr, $($rest:tt)*) => {
        $crate::__structured_message_chain!(
            $crate::StructuredMessage::new($code),
            $($rest)*
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct TestCatalog {
        by_locale: BTreeMap<Locale, BTreeMap<&'static str, &'static str>>,
    }

    impl MessageCatalog for TestCatalog {
        fn get(&self, locale: Locale, key: &str) -> Option<String> {
            self.by_locale
                .get(&locale)
                .and_then(|catalog| catalog.get(key))
                .map(|value| (*value).to_string())
        }
    }

    #[test]
    fn renders_nested_message_with_catalog() {
        let catalog = TestCatalog {
            by_locale: BTreeMap::from([(
                Locale::EnUs,
                BTreeMap::from([("outer", "outer: {message}"), ("inner", "hello {name}")]),
            )]),
        };
        let message = StructuredMessage::new("outer").arg_message(
            "message",
            StructuredMessage::new("inner").arg("name", "ditto"),
        );
        assert_eq!(
            message.render_with_catalog(&catalog, Locale::EnUs),
            "outer: hello ditto"
        );
    }

    #[test]
    fn structured_message_macro_preserves_nested_messages() {
        let message = crate::structured_message!(
            "outer",
            "message" => @message StructuredMessage::new("inner"),
        );
        let nested = message
            .args()
            .iter()
            .find(|arg| arg.name() == "message")
            .and_then(StructuredMessageArg::message_value)
            .map(StructuredMessage::code);
        assert_eq!(nested, Some("inner"));
    }

    #[test]
    fn localized_display_uses_external_catalog() {
        struct Greeting;

        impl LocalizedMessage for Greeting {
            fn render_localized<C>(&self, catalog: &C, locale: Locale) -> String
            where
                C: MessageCatalog + ?Sized,
            {
                render_message(
                    catalog,
                    locale,
                    "greeting",
                    &[MessageArg::new("name", "ditto")],
                )
            }
        }

        let catalog = TestCatalog {
            by_locale: BTreeMap::from([(
                Locale::EnUs,
                BTreeMap::from([("greeting", "hello {name}")]),
            )]),
        };

        assert_eq!(
            localized(&Greeting, &catalog, Locale::EnUs).to_string(),
            "hello ditto"
        );
    }
}
