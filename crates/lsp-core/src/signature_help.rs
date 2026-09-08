//! Parameter hints: what a `textDocument/signatureHelp` response means, and
//! — the part that actually goes wrong — which argument the caret is in.
//!
//! The parsing is a formality. The arithmetic is not: a server reports an
//! active parameter for the position it was *asked* about, and the caret has
//! usually moved on by the time the answer lands, so the tip is only correct
//! if this side can recompute the active argument from the buffer text. That
//! computation has to survive nested calls, commas inside string literals,
//! commas inside generic argument lists, comments, and Rust lifetimes that
//! look exactly like an unterminated character literal.
//!
//! All of it is rules, so none of it belongs in `bridge.rs` or `cpp/`
//! (`docs/architecture/layering.md`).

use serde_json::Value;

use crate::manager::{position_params, LspError, SIGNATURE_HELP_TIMEOUT};

/// One parameter of one signature.
///
/// `range` is the parameter's extent **within its signature's label**, in
/// UTF-16 code units — the offsets the view needs to embolden the active
/// argument inside the one string it paints. The protocol allows a server to
/// give either those offsets or a substring of the label, and both are
/// normalised to the offsets here so the view never learns there were two
/// shapes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterInfo {
    pub label: String,
    pub documentation: Option<String>,
    pub range: Option<(u32, u32)>,
}

/// One overload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureInfo {
    pub label: String,
    pub documentation: Option<String>,
    pub parameters: Vec<ParameterInfo>,
    /// The signature's own `activeParameter`, which the protocol says wins
    /// over the response-level one for *this* signature. Overload sets are
    /// exactly where the two disagree.
    pub active_parameter: Option<usize>,
}

/// A `textDocument/signatureHelp` response, as sent.
///
/// The two `active_*` fields are kept raw and resolved by
/// [`SignatureHelp::resolved_signature`] and
/// [`SignatureHelp::resolved_parameter`], because "as sent" and "as shown"
/// genuinely differ: servers send indices past the end of their own arrays,
/// and clamping at the parse boundary would hide that from the tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureHelp {
    pub signatures: Vec<SignatureInfo>,
    pub active_signature: Option<usize>,
    pub active_parameter: Option<usize>,
}

impl SignatureHelp {
    /// The overload to show. An out-of-range `activeSignature` falls back to
    /// the first, which is what the protocol prescribes for a missing one and
    /// the only defensible reading of an impossible one.
    pub fn resolved_signature(&self) -> Option<&SignatureInfo> {
        let index = self.active_signature.unwrap_or(0);
        self.signatures
            .get(index)
            .or_else(|| self.signatures.first())
    }

    /// Which parameter of the shown overload to embolden.
    ///
    /// The signature's own `activeParameter` wins over the response-level
    /// one — that is the protocol's rule and it matters: an overload set is
    /// reported once with one response-level index, and the overload that
    /// takes fewer arguments has to be able to say so. An index past the end
    /// of *that* signature's parameters is not clamped into a lie; it means
    /// the caret is in an argument this overload does not have, and nothing
    /// is emboldened.
    pub fn resolved_parameter(&self) -> Option<usize> {
        let signature = self.resolved_signature()?;
        let index = signature.active_parameter.or(self.active_parameter)?;
        (index < signature.parameters.len()).then_some(index)
    }
}

/// Parse a `textDocument/signatureHelp` result. `None` for `null`, for a
/// missing or empty `signatures` array, and for anything unparseable — all
/// of which mean "no hint here", not an error.
pub fn parse_signature_help(result: &Value) -> Option<SignatureHelp> {
    let signatures: Vec<SignatureInfo> = result
        .get("signatures")?
        .as_array()?
        .iter()
        .filter_map(signature)
        .collect();
    if signatures.is_empty() {
        return None;
    }
    Some(SignatureHelp {
        signatures,
        active_signature: index(result.get("activeSignature")),
        active_parameter: index(result.get("activeParameter")),
    })
}

fn signature(value: &Value) -> Option<SignatureInfo> {
    let label = value.get("label")?.as_str()?.to_string();
    let parameters = value
        .get("parameters")
        .and_then(Value::as_array)
        .map(|items| parameters(&label, items))
        .unwrap_or_default();
    Some(SignatureInfo {
        label,
        documentation: documentation(value.get("documentation")),
        parameters,
        active_parameter: index(value.get("activeParameter")),
    })
}

/// The parameters of one signature, with every label shape reduced to both a
/// string and a range.
///
/// The substring form is searched for from the end of the previous
/// parameter rather than from the start of the label. The protocol asks for
/// a substring that identifies the parameter unambiguously, and servers
/// routinely send one that does not — `fn f(a, aa)` reports `a` for the
/// first parameter and `aa` for the second, and a naive search for `aa`
/// would still be right while a naive search for a later `a` would not.
/// Walking a cursor forward costs one variable and makes the common
/// duplicate-prefix case come out in the order the parameters are declared.
fn parameters(signature_label: &str, items: &[Value]) -> Vec<ParameterInfo> {
    let units: Vec<u16> = signature_label.encode_utf16().collect();
    let mut cursor = 0usize;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(label) = item.get("label") else {
            continue;
        };
        let (text, range) = match label {
            Value::String(text) => {
                let range = find_utf16(&units, text, cursor);
                if let Some((_, end)) = range {
                    cursor = end as usize;
                }
                (text.clone(), range)
            }
            Value::Array(offsets) => {
                let (Some(start), Some(end)) = (
                    offsets.first().and_then(Value::as_u64),
                    offsets.get(1).and_then(Value::as_u64),
                ) else {
                    continue;
                };
                let (start, end) = (start as u32, end as u32);
                let text = slice_utf16(&units, start, end);
                cursor = end as usize;
                (text, Some((start, end)))
            }
            _ => continue,
        };
        out.push(ParameterInfo {
            label: text,
            documentation: documentation(item.get("documentation")),
            range,
        });
    }
    out
}

/// `MarkupContent` or a bare string, both flattened to the text a tooltip
/// shows. Signature documentation is a sentence, not a document, so no
/// Markdown rendering is attempted here — [`crate::hover::to_tooltip_html`]
/// exists for the surface that needs it.
fn documentation(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Object(_) => value?
            .get("value")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

fn index(value: Option<&Value>) -> Option<usize> {
    value?.as_u64().map(|n| n as usize)
}

fn slice_utf16(units: &[u16], start: u32, end: u32) -> String {
    let start = (start as usize).min(units.len());
    let end = (end as usize).clamp(start, units.len());
    String::from_utf16_lossy(&units[start..end])
}

/// The UTF-16 range of `needle` within `units`, searched from `from`.
fn find_utf16(units: &[u16], needle: &str, from: usize) -> Option<(u32, u32)> {
    let needle: Vec<u16> = needle.encode_utf16().collect();
    if needle.is_empty() || needle.len() > units.len() {
        return None;
    }
    let from = from.min(units.len() - needle.len() + 1);
    let start = (from..=units.len() - needle.len())
        .find(|&i| units[i..i + needle.len()] == needle[..])
        // A server whose substring is not in its own label after the previous
        // parameter may simply have listed them out of order; fall back to a
        // search from the start before giving up.
        .or_else(|| {
            (0..=units.len() - needle.len()).find(|&i| units[i..i + needle.len()] == needle[..])
        })?;
    Some((start as u32, (start + needle.len()) as u32))
}

// -- Where the caret is ----------------------------------------------------

/// The argument list the caret sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallSite {
    /// Byte offset of the `(` that opened the list — the identity of the
    /// call. It changes exactly when the caret enters or leaves a call, which
    /// is when a new request is worth sending.
    pub open_paren: usize,
    /// 0-based index of the argument the caret is in.
    pub parameter: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Bracket {
    Paren,
    Square,
    Curly,
    /// A `<` that *might* open a generic argument list.
    Angle,
}

struct Frame {
    open: usize,
    bracket: Bracket,
    commas: usize,
}

/// Which call's argument list `caret` (a byte offset) is inside, and which
/// argument, by scanning the text before it.
///
/// `None` means the caret is not inside any argument list — which is what
/// typing the closing paren produces, and therefore the dismissal rule.
///
/// The scan is a bracket stack with three deliberate refinements, each of
/// which exists because a naive comma counter gets a real program wrong:
///
/// * **Commas belong to the innermost bracket**, so `f(g(1, 2), 3)` puts the
///   caret before `2` in *g*'s second argument, and `f([1, 2, 3], x)` treats
///   the whole array literal as one argument of `f`.
/// * **String, character and comment contents are skipped**, so `f("a, b", |)`
///   is in argument 1 and not argument 2. A `'` only opens a character
///   literal when a closing `'` follows within a few characters — otherwise
///   it is a Rust lifetime (`f(x: &'a str, |)`), and treating that as a
///   string would swallow the rest of the file.
/// * **`<` is tracked as a bracket**, so the comma in `f(x: Vec<A, B>, |)`
///   is charged to the generic argument list and the caret lands in
///   argument 1.
///
/// ponytail: the `<` rule is a heuristic and cannot be anything else without
/// parsing — `f(a < b, c > d)` counts one argument where a compiler would
/// see two. It is bounded: an unclosed `<` is discarded when its enclosing
/// bracket closes or a `;` ends the statement, so the damage cannot outlive
/// the expression. Upgrade path is tree-sitter, which this crate
/// deliberately does not depend on.
pub fn call_site_at(text: &str, caret: usize) -> Option<CallSite> {
    let caret = caret.min(text.len());
    let bytes = text.as_bytes();
    let mut stack: Vec<Frame> = Vec::new();
    let mut i = 0usize;

    while i < caret {
        match bytes[i] {
            b'"' => {
                i = skip_string(bytes, i, caret, b'"');
                continue;
            }
            b'\'' if is_char_literal(bytes, i, caret) => {
                i = skip_string(bytes, i, caret, b'\'');
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i = memchr(bytes, b'\n', i + 2, caret).map_or(caret, |n| n + 1);
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = skip_block_comment(bytes, i, caret);
                continue;
            }
            b'(' => push(&mut stack, i, Bracket::Paren),
            b'[' => push(&mut stack, i, Bracket::Square),
            b'{' => push(&mut stack, i, Bracket::Curly),
            b'<' => push(&mut stack, i, Bracket::Angle),
            b')' => close(&mut stack, Bracket::Paren),
            b']' => close(&mut stack, Bracket::Square),
            b'}' => close(&mut stack, Bracket::Curly),
            // A `>` closes a generic list if one is open and is otherwise an
            // operator (`->`, `>=`), which the stack is right to ignore.
            b'>' => drop_one_angle(&mut stack),
            // A statement boundary: any `<` still open was a comparison.
            b';' => drop_angles(&mut stack),
            b',' => {
                if let Some(frame) = stack.last_mut() {
                    frame.commas += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }

    stack
        .iter()
        .rev()
        .find(|frame| frame.bracket == Bracket::Paren)
        .map(|frame| CallSite {
            open_paren: frame.open,
            parameter: frame.commas,
        })
}

fn push(stack: &mut Vec<Frame>, open: usize, bracket: Bracket) {
    stack.push(Frame {
        open,
        bracket,
        commas: 0,
    });
}

/// Close `bracket`, discarding any `<` frames that were still open inside
/// it — they were comparisons, not generics.
fn close(stack: &mut Vec<Frame>, bracket: Bracket) {
    drop_angles(stack);
    if matches!(stack.last(), Some(f) if f.bracket == bracket) {
        stack.pop();
    }
}

fn drop_one_angle(stack: &mut Vec<Frame>) {
    if matches!(stack.last(), Some(f) if f.bracket == Bracket::Angle) {
        stack.pop();
    }
}

/// Past the `*/` of the block comment opening at `start`.
fn skip_block_comment(bytes: &[u8], start: usize, to: usize) -> usize {
    let mut i = start + 2;
    while i + 1 < to {
        if bytes[i] == b'*' && bytes[i + 1] == b'/' {
            return i + 2;
        }
        i += 1;
    }
    to
}

fn drop_angles(stack: &mut Vec<Frame>) {
    while matches!(stack.last(), Some(f) if f.bracket == Bracket::Angle) {
        stack.pop();
    }
}

fn memchr(bytes: &[u8], needle: u8, from: usize, to: usize) -> Option<usize> {
    (from..to).find(|&i| bytes[i] == needle)
}

/// Past the closing `delimiter` of the literal opening at `start`, honouring
/// backslash escapes. An unterminated literal consumes the rest of the scan,
/// which is correct: everything after it *is* inside the literal.
fn skip_string(bytes: &[u8], start: usize, to: usize, delimiter: u8) -> usize {
    let mut i = start + 1;
    while i < to {
        match bytes[i] {
            b'\\' => i += 2,
            b if b == delimiter => return i + 1,
            _ => i += 1,
        }
    }
    to
}

/// Is the `'` at `start` a character literal rather than a Rust lifetime?
///
/// A character literal is at most a few bytes long (`'a'`, `'\n'`, `'\u{1F600}'`
/// being the outlier), while a lifetime never closes at all. Looking ahead a
/// bounded distance for a closing quote is the whole rule.
fn is_char_literal(bytes: &[u8], start: usize, to: usize) -> bool {
    const MAX_CHAR_LITERAL: usize = 12;
    let end = (start + MAX_CHAR_LITERAL).min(to);
    let mut i = start + 1;
    while i < end {
        match bytes[i] {
            b'\\' => i += 2,
            b'\'' => return true,
            b'\n' => return false,
            _ => i += 1,
        }
    }
    false
}

// -- When to ask, and when to stop showing --------------------------------

/// The characters a server wants signature help (re)requested on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignatureTriggers {
    pub supported: bool,
    pub trigger: Vec<String>,
    pub retrigger: Vec<String>,
}

/// What a server advertises in its `initialize` result.
///
/// A server with no `signatureHelpProvider` gets no requests at all, not even
/// explicit ones — asking for a method the server does not implement earns a
/// `-32601` and nothing else. A server that advertises the provider but names
/// no trigger characters gets the two every language agrees on, because
/// otherwise the feature would silently never fire.
pub fn parse_signature_triggers(init_result: &Value) -> SignatureTriggers {
    let Some(provider) = init_result.pointer("/capabilities/signatureHelpProvider") else {
        return SignatureTriggers::default();
    };
    let list = |field: &str| -> Vec<String> {
        provider
            .get(field)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut trigger = list("triggerCharacters");
    if trigger.is_empty() {
        trigger = vec!["(".into(), ",".into()];
    }
    SignatureTriggers {
        supported: true,
        trigger,
        retrigger: list("retriggerCharacters"),
    }
}

impl SignatureTriggers {
    fn matches(list: &[String], typed: char) -> bool {
        list.iter().any(|t| t.starts_with(typed))
    }
}

/// Should a `textDocument/signatureHelp` request be sent?
///
/// `explicit` is the user pressing Parameter Info, which asks regardless of
/// what was typed. Everything else is: the caret must be inside an argument
/// list at all — which is what makes the closing paren stop the requests
/// rather than needing a rule of its own — and the character just typed must
/// be one the server named. A retrigger character only fires while a tip is
/// already showing, which is its entire purpose: `)` closing an inner call
/// returns the caret to the outer one and the tip must change to it.
pub fn should_request(
    triggers: &SignatureTriggers,
    text: &str,
    caret: usize,
    explicit: bool,
    showing: bool,
) -> bool {
    if !triggers.supported || call_site_at(text, caret).is_none() {
        return false;
    }
    if explicit {
        return true;
    }
    let Some(typed) = text[..caret.min(text.len())].chars().next_back() else {
        return false;
    };
    SignatureTriggers::matches(&triggers.trigger, typed)
        || (showing && SignatureTriggers::matches(&triggers.retrigger, typed))
}

/// Should a tip that is currently showing be hidden?
///
/// Leaving every argument list — typing the closing paren, or moving the
/// caret out of the call — is the dismissal. Escape is the view's own affair.
pub fn should_dismiss(text: &str, caret: usize) -> bool {
    call_site_at(text, caret).is_none()
}

// C4-followup (#162): request-sending `LspManager` methods for this feature, moved out of
// `manager.rs` once it crossed the file-size ceiling. This file already held the
// parse/rule layer; this is the request-sending half `manager.rs`'s own module doc
// pointed callers to.
impl crate::manager::LspManager {
    /// `textDocument/signatureHelp` for a position in an open document.
    /// `Ok(None)` means the caret is not in a call the server recognises.
    ///
    /// The server's `activeParameter` describes the position it was asked
    /// about; by the time the answer lands the caret may have moved, which
    /// is what [`crate::signature_help::call_site_at`] exists to correct.
    pub fn signature_help(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Option<SignatureHelp>, LspError> {
        let uri = &self.normalize_uri(uri);
        let language_id = self.language_of(uri)?;
        let result = self.request_with_timeout(
            &language_id,
            "textDocument/signatureHelp",
            position_params(uri, line, character),
            SIGNATURE_HELP_TIMEOUT,
        )?;
        Ok(parse_signature_help(&result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn help(value: Value) -> SignatureHelp {
        parse_signature_help(&value).expect("parses")
    }

    #[test]
    fn a_signature_with_offset_parameter_labels_is_parsed() {
        let parsed = help(json!({
            "signatures": [{
                "label": "fn push(&mut self, value: T)",
                "documentation": "Appends an element.",
                "parameters": [
                    {"label": [8, 17]},
                    {"label": [19, 27], "documentation": {"kind": "markdown", "value": "the **value**"}},
                ],
            }],
            "activeSignature": 0,
            "activeParameter": 1,
        }));

        let signature = parsed.resolved_signature().unwrap();
        assert_eq!(
            signature.documentation.as_deref(),
            Some("Appends an element.")
        );
        assert_eq!(signature.parameters[0].label, "&mut self");
        assert_eq!(signature.parameters[1].label, "value: T");
        assert_eq!(signature.parameters[1].range, Some((19, 27)));
        assert_eq!(
            signature.parameters[1].documentation.as_deref(),
            Some("the **value**"),
        );
        assert_eq!(parsed.resolved_parameter(), Some(1));
    }

    #[test]
    fn a_substring_parameter_label_is_resolved_to_the_same_range() {
        let parsed = help(json!({
            "signatures": [{
                "label": "fn push(&mut self, value: T)",
                "parameters": [{"label": "&mut self"}, {"label": "value: T"}],
            }],
            "activeParameter": 1,
        }));

        let signature = parsed.resolved_signature().unwrap();
        assert_eq!(signature.parameters[0].range, Some((8, 17)));
        assert_eq!(
            signature.parameters[1].range,
            Some((19, 27)),
            "both label shapes must reach the view as offsets",
        );
    }

    #[test]
    fn a_duplicate_prefix_substring_resolves_in_declaration_order() {
        let parsed = help(json!({
            "signatures": [{
                "label": "f(a, aa)",
                "parameters": [{"label": "a"}, {"label": "aa"}],
            }],
        }));
        let signature = parsed.resolved_signature().unwrap();
        assert_eq!(signature.parameters[0].range, Some((2, 3)));
        assert_eq!(
            signature.parameters[1].range,
            Some((5, 7)),
            "the second `a` must not be found inside the first parameter",
        );
    }

    #[test]
    fn parameter_ranges_are_counted_in_utf16_code_units() {
        // The emoji is two UTF-16 code units and four UTF-8 bytes; a byte
        // offset here would embolden the wrong half of the label.
        let parsed = help(json!({
            "signatures": [{"label": "f(😀: T, b)", "parameters": [{"label": "b"}]}],
        }));
        assert_eq!(
            parsed.resolved_signature().unwrap().parameters[0].range,
            Some((9, 10))
        );
    }

    #[test]
    fn a_signatures_own_active_parameter_wins_over_the_response_level_one() {
        let parsed = help(json!({
            "signatures": [
                {"label": "f(a, b)", "parameters": [{"label": "a"}, {"label": "b"}]},
                {"label": "f(a)", "parameters": [{"label": "a"}], "activeParameter": 0},
            ],
            "activeSignature": 1,
            "activeParameter": 1,
        }));

        assert_eq!(parsed.resolved_signature().unwrap().label, "f(a)");
        assert_eq!(
            parsed.resolved_parameter(),
            Some(0),
            "the overload that takes one argument says so itself",
        );
    }

    #[test]
    fn an_active_parameter_past_the_overloads_arity_emboldens_nothing() {
        let parsed = help(json!({
            "signatures": [{"label": "f(a)", "parameters": [{"label": "a"}]}],
            "activeParameter": 2,
        }));
        assert_eq!(parsed.resolved_parameter(), None);
    }

    #[test]
    fn an_out_of_range_active_signature_falls_back_to_the_first() {
        let parsed = help(json!({
            "signatures": [{"label": "f(a)", "parameters": [{"label": "a"}]}],
            "activeSignature": 7,
        }));
        assert_eq!(parsed.resolved_signature().unwrap().label, "f(a)");
    }

    #[test]
    fn nothing_to_show_parses_to_none() {
        assert!(parse_signature_help(&Value::Null).is_none());
        assert!(parse_signature_help(&json!({"signatures": []})).is_none());
        assert!(parse_signature_help(&json!({"signatures": [{"no": "label"}]})).is_none());
    }

    // -- the arithmetic ---------------------------------------------------

    fn parameter_at(text: &str, caret: usize) -> Option<usize> {
        call_site_at(text, caret).map(|site| site.parameter)
    }

    #[test]
    fn the_active_argument_of_a_nested_call_at_every_caret_offset() {
        //          0123456789...
        let text = "foo(bar(1, 2), 3)";
        let cases = [
            (0, None),
            (3, None),
            (4, Some(0)),
            (7, Some(0)),
            (8, Some(0)),
            (9, Some(0)),
            (10, Some(1)),
            (11, Some(1)),
            (12, Some(1)),
            (13, Some(0)),
            (14, Some(1)),
            (16, Some(1)),
            (17, None),
        ];
        for (caret, expected) in cases {
            assert_eq!(
                parameter_at(text, caret),
                expected,
                "caret {caret} in `{}|{}`",
                &text[..caret],
                &text[caret..],
            );
        }
    }

    #[test]
    fn the_caret_is_attributed_to_the_innermost_call_not_the_outermost() {
        let text = "foo(bar(1, 2), 3)";
        assert_eq!(call_site_at(text, 11).unwrap().open_paren, 7, "bar's");
        assert_eq!(call_site_at(text, 15).unwrap().open_paren, 3, "foo's");
    }

    #[test]
    fn a_comma_inside_a_string_does_not_advance_the_argument() {
        assert_eq!(parameter_at(r#"f("a, b", x)"#, 10), Some(1));
        assert_eq!(parameter_at(r#"f("a, b, c")"#, 11), Some(0));
        assert_eq!(
            parameter_at(r#"f("\", x", y)"#, 11),
            Some(1),
            "an escaped quote does not end the string",
        );
        assert_eq!(parameter_at("f('a', ',', x)", 13), Some(2));
    }

    #[test]
    fn a_lifetime_is_not_an_unterminated_character_literal() {
        // Treating `'a` as a string would swallow the rest of the call and
        // report argument 0 forever.
        assert_eq!(parameter_at("f(x: &'a str, y)", 14), Some(1));
    }

    #[test]
    fn a_comma_inside_a_generic_argument_list_does_not_advance_the_argument() {
        assert_eq!(parameter_at("f(x: Vec<A, B>, )", 16), Some(1));
        assert_eq!(parameter_at("f(x: Map<A, Vec<B, C>>, )", 24), Some(1));
    }

    #[test]
    fn a_comma_inside_a_collection_literal_belongs_to_the_literal() {
        assert_eq!(parameter_at("f([1, 2, 3], x)", 13), Some(1));
        assert_eq!(parameter_at("f({a: 1, b: 2}, x)", 16), Some(1));
    }

    #[test]
    fn a_comma_in_a_comment_does_not_advance_the_argument() {
        assert_eq!(parameter_at("f(a /* x, y */, b)", 16), Some(1));
        assert_eq!(parameter_at("f(a, // x, y\n b)", 14), Some(1));
    }

    #[test]
    fn an_unclosed_comparison_cannot_outlive_its_statement() {
        // ponytail's known ceiling, pinned: `a < b` inside the call is
        // mis-read while the call is open, but the `;` and the closing paren
        // both clear it, so the next call starts from a clean stack.
        assert_eq!(parameter_at("g(a < b, c); f(x, ", 18), Some(1));
    }

    // -- when to ask ------------------------------------------------------

    fn triggers() -> SignatureTriggers {
        parse_signature_triggers(&json!({"capabilities": {"signatureHelpProvider": {
            "triggerCharacters": ["(", ","], "retriggerCharacters": [")"],
        }}}))
    }

    #[test]
    fn a_server_without_the_provider_is_never_asked() {
        let none = parse_signature_triggers(&json!({"capabilities": {}}));
        assert!(!none.supported);
        assert!(!should_request(&none, "f(", 2, true, false));
    }

    #[test]
    fn a_provider_with_no_trigger_characters_still_fires() {
        let defaults =
            parse_signature_triggers(&json!({"capabilities": {"signatureHelpProvider": {}}}));
        assert!(should_request(&defaults, "f(", 2, false, false));
    }

    #[test]
    fn the_open_paren_and_the_comma_ask_and_ordinary_typing_does_not() {
        let t = triggers();
        assert!(should_request(&t, "f(", 2, false, false));
        assert!(should_request(&t, "f(a,", 4, false, true));
        assert!(!should_request(&t, "f(ab", 4, false, true));
        assert!(
            should_request(&t, "f(ab", 4, true, false),
            "Parameter Info asks wherever the caret is inside a call",
        );
    }

    #[test]
    fn the_closing_paren_dismisses_the_tip_instead_of_asking_again() {
        let t = triggers();
        assert!(should_dismiss("f(a)", 4));
        assert!(!should_request(&t, "f(a)", 4, true, true));
    }

    #[test]
    fn closing_an_inner_call_retriggers_for_the_outer_one() {
        let t = triggers();
        let text = "f(g(1)";
        assert!(!should_dismiss(text, 6), "still inside f");
        assert!(
            should_request(&t, text, 6, false, true),
            "the tip must switch from g's signature back to f's",
        );
        assert!(
            !should_request(&t, text, 6, false, false),
            "a retrigger character only fires while a tip is showing",
        );
    }
}
