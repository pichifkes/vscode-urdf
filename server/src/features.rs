#![allow(dead_code, unused_imports)]

use tower_lsp::lsp_types::*;

use crate::document::{Document, Joint, NamedItem, NameRef};

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
// 4. completion
// ---------------------------------------------------------------------------

pub fn completion(doc: &Document, pos: Position, text: &str) -> Vec<CompletionItem> {
    // Extract the portion of the current line up to the cursor column.
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

    // link="<partial>  → offer link names
    let link_re = regex_match(prefix, r#"link\s*=\s*"[^"]*$"#);
    // joint="<partial> → offer joint names
    let joint_re = regex_match(prefix, r#"joint\s*=\s*"[^"]*$"#);
    // ${<partial>      → offer xacro property names
    let xacro_re = regex_match(prefix, r#"\$\{[^}]*$"#);

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
        vec![]
    }
}

/// Minimal regex-free pattern matching for the three completion triggers.
/// Rather than pulling in the `regex` crate (not in Cargo.toml), we use
/// simple string searches that are equivalent for our well-defined patterns.
///
/// Pattern semantics:
///   `link\s*=\s*"[^"]*$`  → the prefix ends with  link="…  (no closing quote)
///   `joint\s*=\s*"[^"]*$` → the prefix ends with  joint="…
///   `\$\{[^}]*$`          → the prefix ends with  ${…  (no closing brace)
fn regex_match(prefix: &str, pattern: &str) -> bool {
    match pattern {
        r#"link\s*=\s*"[^"]*$"# => {
            // Find last occurrence of `link` followed (with optional spaces) by `="`
            find_attr_open(prefix, "link")
        }
        r#"joint\s*=\s*"[^"]*$"# => {
            find_attr_open(prefix, "joint")
        }
        r#"\$\{[^}]*$"# => {
            // There must be a `${` after the last `}` (if any)
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
