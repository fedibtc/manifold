use serde_json::Value;

use super::check;

fn nested(depth: usize, object: bool, leaf: &str) -> String {
    let (open, close) = if object { ("{\"x\":", "}") } else { ("[", "]") };
    format!("{}{leaf}{}", open.repeat(depth), close.repeat(depth))
}

/// The decoder, not the preflight, still decides acceptance and decoded values.
fn assert_decoder_unchanged(input: &str) {
    let baseline = serde_json::from_str::<Value>(input);
    let guarded = check(input)
        .ok()
        .and_then(|()| serde_json::from_str::<Value>(input).ok());
    assert_eq!(baseline.ok(), guarded, "input: {input:?}");
}

#[test]
fn container_boundary_matches_default_decoder() {
    for object in [false, true] {
        for depth in [0, 1, 125, 126, 127, 128, 129] {
            for leaf in ["0", "null", "\"[]{}\""] {
                let input = nested(depth, object, leaf);
                assert_eq!(check(&input).is_ok(), depth <= 127);
                assert_eq!(serde_json::from_str::<Value>(&input).is_ok(), depth <= 127);
                assert_decoder_unchanged(&input);
            }
        }
    }
    // Siblings release their depth budget, including mixed containers.
    let child = nested(125, true, "[]");
    assert!(check(&format!("[{child},{child}]")).is_ok());
    assert_decoder_unchanged(&format!("[{child},{child}]"));
    assert!(check(&format!("[[{child}]]")).is_err());
}

#[test]
fn strings_and_escape_parity_do_not_create_containers() {
    for text in [
        "[]{}".repeat(1_000),
        "\\\"[{]}".repeat(1_000),
        "é🦀\\\n\t\"".repeat(100),
    ] {
        let encoded = serde_json::to_string(&text).unwrap();
        let input = nested(127, false, &encoded);
        assert!(check(&input).is_ok());
        assert_decoder_unchanged(&input);
    }
    // An even backslash run ends at the following quote; an odd one shields it.
    for count in 0..12 {
        let text = format!("{}\"{}", "\\".repeat(count), "[".repeat(128));
        let encoded = serde_json::to_string(&text).unwrap();
        assert!(check(&encoded).is_ok());
        assert_decoder_unchanged(&encoded);
        let input = format!("[{encoded},{}]", nested(127, false, "0"));
        assert!(check(&input).is_err());
    }
}

#[test]
fn preflight_does_not_replace_validation_or_normalize_values() {
    for input in [
        "",
        "]",
        "}{",
        "[}",
        "[0,]",
        "{\"x\":}",
        "\"unterminated",
        r#""\q""#,
        r#""\uD800""#,
        "01",
        "true false",
        "{} trailing",
        "null",
        "9007199254740993",
        "1e400",
        "-0.0",
        r#"{"x":1,"\u0078":2}"#,
        r#"{"$serde_json::private::Number":"1"}"#,
        r#"{"payload":{"future":{"value":[1,true,null]}},"proof":"fixture"}"#,
    ] {
        // Deliberately includes invalid JSON and duplicate keys. This helper
        // does not impose a duplicate or numeric policy on the chosen decoder.
        assert!(check(input).is_ok());
        assert_decoder_unchanged(input);
    }
}

#[test]
fn deterministic_mutations_preserve_decoder_acceptance() {
    let seeds = [
        nested(127, false, "0"),
        nested(127, true, "\"escaped\\\"[]\""),
        r#"{"a":[null,true,{"b":"\\\"[{]"}],"c":1.25}"#.to_owned(),
    ];
    for seed in seeds {
        for index in 0..seed.len() {
            assert_decoder_unchanged(&seed[..index]);
            for replacement in *b"[]{}\"\\,0" {
                let mut mutated = seed.as_bytes().to_vec();
                mutated[index] = replacement;
                assert_decoder_unchanged(std::str::from_utf8(&mutated).unwrap());
            }
        }
    }
}

#[test]
fn adversarial_depth_is_rejected() {
    // Historical reproducer shape: 262,001 bytes, 131,000 nested arrays.
    // No old recursive parser or production integration is implied here.
    let input = nested(131_000, false, "0");
    assert!(check(&input).is_err());
    // Rejection does not require reaching a scalar or closing delimiters.
    assert!(check(&input[..128]).is_err());
}
