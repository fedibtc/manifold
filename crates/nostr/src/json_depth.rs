//! Allocation-free depth preflight for a subsequent JSON decoder.
//!
//! This helper is not yet used by advertisement admission. Call it on the full
//! input before any `RawValue` scan when integrating a recursive raw-token
//! decoder: checking after that scan cannot avoid its scratch allocation.

#[cfg(test)]
mod tests;

/// Reject JSON nesting beyond 127 simultaneously open arrays and objects.
///
/// This matches serde_json's default container budget. Strings and escaped
/// bytes do not contribute to depth. The scan uses constant space and stops at
/// the first excessive opening delimiter; accepted input takes one full scan.
///
/// This is only a prefilter, not JSON validation. Always pass successful input
/// to the real decoder, which remains responsible for syntax, duplicate member
/// names, number handling, and schema checks. Malformed input can pass this
/// check, and its error precedence may change when this check rejects it.
/// This does not bound input size, container width, or downstream allocations.
///
/// # Errors
///
/// Returns a static error when the lexical container depth exceeds 127.
pub fn check(input: &str) -> Result<(), &'static str> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for byte in input.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else {
            match byte {
                b'"' => in_string = true,
                b'[' | b'{' => {
                    depth += 1;
                    if depth > 127 {
                        return Err("JSON recursion limit exceeded");
                    }
                }
                // Unmatched closers are invalid JSON; let the decoder report it.
                b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    Ok(())
}
