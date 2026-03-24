#![forbid(unsafe_code)]

//! 具有显式自由格式与目录语义的结构化文本。
//!
//! 这个 crate 处理的是“可渲染的结构化文本原语”，不是 IM 消息、进程间通信消息或
//! 事件总线消息。它的核心模型只有两类：
//!
//! - `CatalogText`：由稳定 `code` 和命名类型化参数组成
//! - `StructuredText::freeform(...)`：已经是最终用户可见原文的自由格式文本
//!
//! 构造规则：
//!
//! - 对于字面量目录代码和字面量文本参数名称，且参数值不是嵌套结构化文本时，使用
//!   [`structured_text!`]。
//! - 当任何代码、参数名称或嵌套文本来自运行时数据时，使用
//!   [`try_structured_text!`]、[`CatalogText::try_new`] 或检查方法。
//! - 仅当实际有效负载已经是原始用户面向文本且没有目录键需要稍后解析时，才使用
//!   [`StructuredText::freeform`]。

mod macros;
mod render;
mod scalar;
mod serialize;
mod text;
mod validation;

pub use scalar::{StructuredTextScalarArg, StructuredTextScalarValue};
pub use text::{
    CatalogArgRef, CatalogArgValueRef, CatalogText, CatalogTextRef, StructuredText,
    StructuredTextRef,
};
pub use validation::StructuredTextValidationError;

#[doc(hidden)]
pub use validation::{__structured_text_component_is_valid, __structured_text_literals_equal};
