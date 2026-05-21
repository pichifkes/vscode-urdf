use tower_lsp::lsp_types::*;

use crate::document::Document;

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn pos_in_range(pos: Position, range: Range) -> bool {
    let line = pos.line;
    let ch = pos.character;
    if line < range.start.line || line > range.end.line {
        return false;
    }
    if line == range.start.line && ch < range.start.character {
        return false;
    }
    if line == range.end.line && ch > range.end.character {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// 1. document_symbols
// ---------------------------------------------------------------------------

pub fn document_symbols(doc: &Document) -> Vec<DocumentSymbol> {
    let mut symbols: Vec<DocumentSymbol> = Vec::new();

    for link in &doc.links {
        #[allow(deprecated)]
        symbols.push(DocumentSymbol {
            name: format!("link: {}", link.name),
            detail: None,
            kind: SymbolKind::MODULE,
            range: link.range,
            selection_range: link.range,
            children: None,
            tags: None,
            deprecated: None,
        });
    }

    for joint in &doc.joints {
        #[allow(deprecated)]
        symbols.push(DocumentSymbol {
            name: format!("joint: {}", joint.name),
            detail: joint.joint_type.clone(),
            kind: SymbolKind::EVENT,
            range: joint.range,
            selection_range: joint.range,
            children: None,
            tags: None,
            deprecated: None,
        });
    }

    for material in &doc.materials {
        #[allow(deprecated)]
        symbols.push(DocumentSymbol {
            name: format!("material: {}", material.name),
            detail: None,
            kind: SymbolKind::CONSTANT,
            range: material.range,
            selection_range: material.range,
            children: None,
            tags: None,
            deprecated: None,
        });
    }

    for prop in &doc.xacro_properties {
        #[allow(deprecated)]
        symbols.push(DocumentSymbol {
            name: format!("property: {}", prop.name),
            detail: None,
            kind: SymbolKind::VARIABLE,
            range: prop.range,
            selection_range: prop.range,
            children: None,
            tags: None,
            deprecated: None,
        });
    }

    symbols
}

// ---------------------------------------------------------------------------
// 2. goto_definition
// ---------------------------------------------------------------------------

pub fn goto_definition(doc: &Document, pos: Position) -> Option<Range> {
    for joint in &doc.joints {
        // parent → resolve in doc.links
        if let Some(ref parent_ref) = joint.parent {
            if pos_in_range(pos, parent_ref.range) {
                return doc
                    .links
                    .iter()
                    .find(|l| l.name == parent_ref.name)
                    .map(|l| l.range);
            }
        }

        // child → resolve in doc.links
        if let Some(ref child_ref) = joint.child {
            if pos_in_range(pos, child_ref.range) {
                return doc
                    .links
                    .iter()
                    .find(|l| l.name == child_ref.name)
                    .map(|l| l.range);
            }
        }

        // mimic → resolve in doc.joints
        if let Some(ref mimic_ref) = joint.mimic {
            if pos_in_range(pos, mimic_ref.range) {
                return doc
                    .joints
                    .iter()
                    .find(|j| j.name == mimic_ref.name)
                    .map(|j| j.range);
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// 3. hover
// ---------------------------------------------------------------------------

fn hover_markdown(value: String) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: None,
    }
}

pub fn hover(doc: &Document, pos: Position) -> Option<Hover> {
    // Check NameRefs inside joints first (parent / child / mimic).
    // We resolve the target item and return hover for it.
    for joint in &doc.joints {
        if let Some(ref parent_ref) = joint.parent {
            if pos_in_range(pos, parent_ref.range) {
                return doc
                    .links
                    .iter()
                    .find(|l| l.name == parent_ref.name)
                    .map(|l| hover_markdown(format!("**Link** `{}`", l.name)));
            }
        }

        if let Some(ref child_ref) = joint.child {
            if pos_in_range(pos, child_ref.range) {
                return doc
                    .links
                    .iter()
                    .find(|l| l.name == child_ref.name)
                    .map(|l| hover_markdown(format!("**Link** `{}`", l.name)));
            }
        }

        if let Some(ref mimic_ref) = joint.mimic {
            if pos_in_range(pos, mimic_ref.range) {
                return doc.joints.iter().find(|j| j.name == mimic_ref.name).map(
                    |j| {
                        let type_str = j
                            .joint_type
                            .as_deref()
                            .unwrap_or("unknown type");
                        hover_markdown(format!(
                            "**Joint** `{}` *(type: {})*",
                            j.name, type_str
                        ))
                    },
                );
            }
        }
    }

    // Check links
    for link in &doc.links {
        if pos_in_range(pos, link.range) {
            return Some(hover_markdown(format!("**Link** `{}`", link.name)));
        }
    }

    // Check joints
    for joint in &doc.joints {
        if pos_in_range(pos, joint.range) {
            let type_str = joint
                .joint_type
                .as_deref()
                .unwrap_or("unknown type");
            return Some(hover_markdown(format!(
                "**Joint** `{}` *(type: {})*",
                joint.name, type_str
            )));
        }
    }

    // Check materials
    for material in &doc.materials {
        if pos_in_range(pos, material.range) {
            return Some(hover_markdown(format!("**Material** `{}`", material.name)));
        }
    }

    // Check xacro properties
    for prop in &doc.xacro_properties {
        if pos_in_range(pos, prop.range) {
            return Some(hover_markdown(format!("**Property** `{}`", prop.name)));
        }
    }

    None
}

// ---------------------------------------------------------------------------
// name_at / range_and_name_at helpers + ItemKind
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    Link,
    Joint,
}

/// Returns the resolved (target) name and its kind for whatever is at `pos`.
/// For a NameRef the returned name is the TARGET name (the thing being referenced).
fn name_at(doc: &Document, pos: Position) -> Option<(String, ItemKind)> {
    // 1. Walk joints: check NameRef ranges first.
    for joint in &doc.joints {
        if let Some(ref parent_ref) = joint.parent {
            if pos_in_range(pos, parent_ref.range) {
                return Some((parent_ref.name.clone(), ItemKind::Link));
            }
        }
        if let Some(ref child_ref) = joint.child {
            if pos_in_range(pos, child_ref.range) {
                return Some((child_ref.name.clone(), ItemKind::Link));
            }
        }
        if let Some(ref mimic_ref) = joint.mimic {
            if pos_in_range(pos, mimic_ref.range) {
                return Some((mimic_ref.name.clone(), ItemKind::Joint));
            }
        }
    }

    // 2. Walk doc.links
    for link in &doc.links {
        if pos_in_range(pos, link.range) {
            return Some((link.name.clone(), ItemKind::Link));
        }
    }

    // 3. Walk doc.joints
    for joint in &doc.joints {
        if pos_in_range(pos, joint.range) {
            return Some((joint.name.clone(), ItemKind::Joint));
        }
    }

    // 4. Materials and xacro_properties are not renameable yet.
    None
}

/// Returns the range WHERE THE CURSOR IS and the name at that range.
/// Checks NameRefs first, then NamedItems.
fn range_and_name_at(doc: &Document, pos: Position) -> Option<(Range, String)> {
    // NameRefs in joints first.
    for joint in &doc.joints {
        if let Some(ref parent_ref) = joint.parent {
            if pos_in_range(pos, parent_ref.range) {
                return Some((parent_ref.range, parent_ref.name.clone()));
            }
        }
        if let Some(ref child_ref) = joint.child {
            if pos_in_range(pos, child_ref.range) {
                return Some((child_ref.range, child_ref.name.clone()));
            }
        }
        if let Some(ref mimic_ref) = joint.mimic {
            if pos_in_range(pos, mimic_ref.range) {
                return Some((mimic_ref.range, mimic_ref.name.clone()));
            }
        }
    }

    // NamedItems.
    for link in &doc.links {
        if pos_in_range(pos, link.range) {
            return Some((link.range, link.name.clone()));
        }
    }
    for joint in &doc.joints {
        if pos_in_range(pos, joint.range) {
            return Some((joint.range, joint.name.clone()));
        }
    }

    None
}

// ---------------------------------------------------------------------------
// 5. references
// ---------------------------------------------------------------------------

pub fn references(doc: &Document, pos: Position) -> Vec<Range> {
    let (name, kind) = match name_at(doc, pos) {
        Some(v) => v,
        None => return vec![],
    };

    let mut ranges: Vec<Range> = Vec::new();

    match kind {
        ItemKind::Link => {
            // Definition range from doc.links
            if let Some(link) = doc.links.iter().find(|l| l.name == name) {
                ranges.push(link.range);
            }
            // Every parent/child NameRef in doc.joints with that name
            for joint in &doc.joints {
                if let Some(ref parent_ref) = joint.parent {
                    if parent_ref.name == name {
                        ranges.push(parent_ref.range);
                    }
                }
                if let Some(ref child_ref) = joint.child {
                    if child_ref.name == name {
                        ranges.push(child_ref.range);
                    }
                }
            }
        }
        ItemKind::Joint => {
            // Definition range from doc.joints
            if let Some(joint) = doc.joints.iter().find(|j| j.name == name) {
                ranges.push(joint.range);
            }
            // Every mimic NameRef with that name
            for joint in &doc.joints {
                if let Some(ref mimic_ref) = joint.mimic {
                    if mimic_ref.name == name {
                        ranges.push(mimic_ref.range);
                    }
                }
            }
        }
    }

    ranges
}

// ---------------------------------------------------------------------------
// 6. prepare_rename
// ---------------------------------------------------------------------------

pub fn prepare_rename(doc: &Document, pos: Position) -> Option<(Range, String)> {
    range_and_name_at(doc, pos)
}

// ---------------------------------------------------------------------------
// 7. rename
// ---------------------------------------------------------------------------

pub fn rename(doc: &Document, pos: Position, new_name: &str) -> Vec<(Range, String)> {
    references(doc, pos)
        .into_iter()
        .map(|r| (r, new_name.to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// 4. completion
// ---------------------------------------------------------------------------

pub fn completion(doc: &Document, pos: Position, text: &str) -> Vec<CompletionItem> {
    let line_text: &str = text
        .lines()
        .nth(pos.line as usize)
        .unwrap_or("");
    let col = pos.character as usize;
    let prefix = if col <= line_text.len() {
        &line_text[..col]
    } else {
        line_text
    };

    let link_re      = regex_match(prefix, r#"link\s*=\s*"[^"]*$"#);
    let joint_re     = regex_match(prefix, r#"joint\s*=\s*"[^"]*$"#);
    let reference_re = regex_match(prefix, r#"reference\s*=\s*"[^"]*$"#);
    let xacro_re     = regex_match(prefix, r#"\$\{[^}]*$"#);

    if link_re {
        doc.links
            .iter()
            .map(|l| CompletionItem {
                label: l.name.clone(),
                kind: Some(CompletionItemKind::REFERENCE),
                ..CompletionItem::default()
            })
            .collect()
    } else if joint_re {
        doc.joints
            .iter()
            .map(|j| CompletionItem {
                label: j.name.clone(),
                kind: Some(CompletionItemKind::REFERENCE),
                ..CompletionItem::default()
            })
            .collect()
    } else if reference_re {
        doc.links
            .iter()
            .map(|l| CompletionItem {
                label: l.name.clone(),
                kind: Some(CompletionItemKind::REFERENCE),
                detail: Some("link".to_string()),
                ..CompletionItem::default()
            })
            .chain(doc.joints.iter().map(|j| CompletionItem {
                label: j.name.clone(),
                kind: Some(CompletionItemKind::REFERENCE),
                detail: Some("joint".to_string()),
                ..CompletionItem::default()
            }))
            .collect()
    } else if xacro_re {
        doc.xacro_properties
            .iter()
            .map(|p| CompletionItem {
                label: p.name.clone(),
                kind: Some(CompletionItemKind::VARIABLE),
                ..CompletionItem::default()
            })
            .collect()
    } else {
        // <partial inside a <gazebo> block → offer Gazebo property element names
        let cursor_offset: usize = text
            .lines()
            .take(pos.line as usize)
            .map(|l| l.len() + 1)
            .sum::<usize>()
            + col.min(line_text.len());
        if inside_gazebo_block(text, cursor_offset) && is_element_name_trigger(prefix) {
            crate::diagnostics::GAZEBO_PROP_NAMES
                .iter()
                .map(|name| CompletionItem {
                    label: name.to_string(),
                    kind: Some(CompletionItemKind::PROPERTY),
                    ..CompletionItem::default()
                })
                .collect()
        } else {
            vec![]
        }
    }
}

fn regex_match(prefix: &str, pattern: &str) -> bool {
    match pattern {
        r#"link\s*=\s*"[^"]*$"#      => find_attr_open(prefix, "link"),
        r#"joint\s*=\s*"[^"]*$"#     => find_attr_open(prefix, "joint"),
        r#"reference\s*=\s*"[^"]*$"# => find_attr_open(prefix, "reference"),
        r#"\$\{[^}]*$"# => {
            if let Some(open) = prefix.rfind("${") {
                let close = prefix.rfind('}').unwrap_or(0);
                open > close
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Return true if `prefix` contains  `<attr>\s*=\s*"` with no subsequent `"`
/// (i.e. we are inside the opening quote of an attribute named `attr`).
fn find_attr_open(prefix: &str, attr: &str) -> bool {
    // Walk backwards through all occurrences of the attribute name.
    let mut search = prefix;
    loop {
        let Some(pos) = search.find(attr) else {
            return false;
        };
        let after_attr = &search[pos + attr.len()..];
        // Skip optional whitespace
        let after_ws = after_attr.trim_start_matches(|c: char| c == ' ' || c == '\t');
        if let Some(rest) = after_ws.strip_prefix('=') {
            let after_eq = rest.trim_start_matches(|c: char| c == ' ' || c == '\t');
            if let Some(after_quote) = after_eq.strip_prefix('"') {
                // We are inside the quote only if there is no closing `"` after it.
                if !after_quote.contains('"') {
                    return true;
                }
            }
        }
        // Advance past this occurrence and keep looking.
        search = &search[pos + attr.len()..];
        if search.is_empty() {
            return false;
        }
    }
}

/// True when the cursor byte offset falls inside an open `<gazebo …>` block.
fn inside_gazebo_block(text: &str, offset: usize) -> bool {
    let before = &text[..offset.min(text.len())];
    let last_open  = before.rfind("<gazebo");
    let last_close = before.rfind("</gazebo");
    match (last_open, last_close) {
        (Some(open), Some(close)) => open > close,
        (Some(_), None)           => true,
        _                         => false,
    }
}

/// True when the line prefix ends with `<` or `<partial_identifier` (element-name
/// trigger) but not a closing tag (`</`).
fn is_element_name_trigger(prefix: &str) -> bool {
    let trimmed = prefix.trim_end();
    if let Some(lt) = trimmed.rfind('<') {
        let after = &trimmed[lt + 1..];
        !after.starts_with('/') && after.chars().all(|c| c.is_alphanumeric() || c == '_')
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// 7. inlay hints
// ---------------------------------------------------------------------------

/// Scan `text` for `${...}` expressions overlapping `range` and produce an
/// inline hint showing the evaluated value right after the closing `}`.
pub fn inlay_hints(doc: &Document, text: &str, range: Range) -> Vec<InlayHint> {
    let mut hints = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;

    while i + 1 < bytes.len() {
        if bytes[i] != b'$' || bytes[i + 1] != b'{' {
            i += 1;
            continue;
        }
        let inner_start = i + 2;
        let Some(rel) = bytes[inner_start..].iter().position(|&b| b == b'}') else {
            break;
        };
        let inner_end = inner_start + rel;
        let close = inner_end + 1;
        let expr = &text[inner_start..inner_end];

        let position = crate::document::byte_offset_to_position(text, close);
        if position_in_range(position, range) {
            if let Some(v) = crate::xacro_eval::eval(expr, &doc.xacro_properties) {
                hints.push(InlayHint {
                    position,
                    label: InlayHintLabel::String(format!(
                        "={}",
                        crate::xacro_eval::format_value(v)
                    )),
                    kind: Some(InlayHintKind::TYPE),
                    text_edits: None,
                    tooltip: None,
                    padding_left: Some(true),
                    padding_right: None,
                    data: None,
                });
            }
        }
        i = close;
    }

    hints
}

fn position_in_range(pos: Position, range: Range) -> bool {
    let after_start = pos.line > range.start.line
        || (pos.line == range.start.line && pos.character >= range.start.character);
    let before_end = pos.line < range.end.line
        || (pos.line == range.end.line && pos.character <= range.end.character);
    after_start && before_end
}
