use serde_json::json;

use crate::{
    CatalogText, StructuredText, StructuredTextValidationError, structured_text,
    try_structured_text,
};

#[test]
fn catalog_text_keeps_args_sorted_for_lookup_and_iteration() {
    let text = structured_text!(
        "demo.message",
        "zeta" => true,
        "alpha" => 7_u32,
        "beta" => "ready",
    );

    let catalog = text.as_catalog().expect("catalog text");
    let arg_names: Vec<_> = catalog.iter_args().map(|arg| arg.name()).collect();
    assert_eq!(arg_names, vec!["alpha", "beta", "zeta"]);
    assert_eq!(catalog.unsigned_arg("alpha"), Some(7));
    assert_eq!(catalog.text_arg("beta"), Some("ready"));
    assert_eq!(catalog.bool_arg("zeta"), Some(true));
}

#[test]
fn display_and_diagnostic_display_preserve_freeform_boundary() {
    let freeform = StructuredText::freeform("literal user text");
    assert_eq!(freeform.to_string(), "literal user text");
    assert_eq!(
        freeform.diagnostic_display().to_string(),
        "@freeform(\"literal user text\")"
    );

    let nested = try_structured_text!("demo.wrapper", "body" => @text freeform.clone())
        .expect("nested text");
    assert_eq!(
        nested.to_string(),
        "demo.wrapper {body=@freeform(\"literal user text\")}"
    );
    assert_eq!(nested.diagnostic_display().to_string(), nested.to_string());
}

#[test]
fn serde_serialization_keeps_typed_values_and_nested_text_shape() {
    let nested = StructuredText::freeform("leaf");
    let text =
        try_structured_text!("demo.payload", "count" => 2_u64, "child" => @text nested.clone())
            .expect("structured text");

    let value = serde_json::to_value(&text).expect("serialize structured text");
    assert_eq!(
        value,
        json!({
            "kind": "catalog",
            "code": "demo.payload",
            "args": [
                {
                    "name": "child",
                    "value": {
                        "kind": "nested_text",
                        "value": {
                            "kind": "freeform",
                            "text": "leaf",
                        }
                    }
                },
                {
                    "name": "count",
                    "value": {
                        "kind": "unsigned",
                        "value": "2",
                    }
                }
            ]
        })
    );
}

#[test]
fn runtime_validation_rejects_duplicate_names_and_excessive_nesting() {
    let mut text = CatalogText::try_new("demo.payload").expect("valid code");
    text.try_with_value_arg("name", "first")
        .expect("first arg should succeed");
    let duplicate = text
        .try_with_value_arg("name", "second")
        .expect_err("duplicate arg should fail");
    assert_eq!(
        duplicate,
        StructuredTextValidationError::DuplicateArgName("name".to_string())
    );

    let mut nested = StructuredText::freeform("leaf");
    for _ in 0..31 {
        nested = try_structured_text!("demo.node", "child" => @text nested)
            .expect("nesting within limit");
    }
    let too_deep = try_structured_text!("demo.root", "child" => @text nested)
        .expect_err("nesting past the documented limit should fail");
    assert_eq!(
        too_deep,
        StructuredTextValidationError::NestingTooDeep { max_depth: 32 }
    );
}
