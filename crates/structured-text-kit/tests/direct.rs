use structured_text_kit::{
    StructuredText, StructuredTextValidationError, structured_text, try_structured_text,
};

#[test]
fn catalog_args_are_sorted_and_lookup_is_typed() {
    let text = structured_text!(
        "demo.message",
        "zeta" => 7u64,
        "alpha" => "hello",
        "flag" => true
    );

    let catalog = text.as_catalog().expect("catalog text");
    let names: Vec<_> = catalog.iter_args().map(|arg| arg.name()).collect();
    assert_eq!(names, vec!["alpha", "flag", "zeta"]);
    assert_eq!(catalog.text_arg("alpha"), Some("hello"));
    assert_eq!(catalog.bool_arg("flag"), Some(true));
    assert_eq!(catalog.unsigned_arg("zeta"), Some(7));
}

#[test]
fn display_and_diagnostic_rendering_stay_distinct_for_freeform() {
    let freeform = StructuredText::freeform("hello\nworld");

    assert_eq!(freeform.freeform_text(), Some("hello\nworld"));
    assert_eq!(format!("{freeform}"), "hello\nworld");
    assert_eq!(
        format!("{}", freeform.diagnostic_display()),
        "@freeform(\"hello\\nworld\")"
    );
}

#[test]
fn serde_preserves_catalog_and_nested_shapes() {
    let nested = StructuredText::freeform("leaf");
    let text = try_structured_text!(
        "demo.outer",
        "enabled" => true,
        "child" => @text nested,
        "count" => 7u64
    )
    .expect("valid structured text");

    let value = serde_json::to_value(&text).expect("serialize structured text");
    assert_eq!(value["kind"], "catalog");
    assert_eq!(value["code"], "demo.outer");
    assert_eq!(value["args"][0]["name"], "child");
    assert_eq!(value["args"][0]["value"]["kind"], "nested_text");
    assert_eq!(value["args"][0]["value"]["value"]["kind"], "freeform");
    assert_eq!(value["args"][0]["value"]["value"]["text"], "leaf");
    assert_eq!(value["args"][1]["name"], "count");
    assert_eq!(value["args"][1]["value"]["kind"], "unsigned");
    assert_eq!(value["args"][1]["value"]["value"], "7");
    assert_eq!(value["args"][2]["name"], "enabled");
    assert_eq!(value["args"][2]["value"]["kind"], "bool");
    assert_eq!(value["args"][2]["value"]["value"], true);
}

#[test]
fn nested_text_validation_rejects_excessive_depth() {
    let mut nested = StructuredText::freeform("leaf");
    let mut err = None;

    for _ in 0..64 {
        match try_structured_text!("demo.node", "child" => @text nested.clone()) {
            Ok(next) => nested = next,
            Err(next_err) => {
                err = Some(next_err);
                break;
            }
        }
    }

    assert_eq!(
        err,
        Some(StructuredTextValidationError::NestingTooDeep { max_depth: 32 })
    );
}
