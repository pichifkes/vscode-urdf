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
                match prefix.rfind('}') {
                    Some(close) => open > close,
                    None => true,
                }
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

// ---------------------------------------------------------------------------
// 8. document colors (color swatches + picker)
// ---------------------------------------------------------------------------

/// Find every `<color rgba="r g b a"/>` (or `<material name="..."><color/>`)
/// and return a ColorInformation so VS Code can render a clickable swatch.
pub fn document_colors(text: &str) -> Vec<ColorInformation> {
    let Ok(xml) = roxmltree::Document::parse(text) else { return Vec::new(); };
    let mut out = Vec::new();
    walk_colors(xml.root_element(), text, &mut out);
    out
}

fn walk_colors(node: roxmltree::Node, text: &str, out: &mut Vec<ColorInformation>) {
    if node.is_element() && node.tag_name().name() == "color" {
        if let Some(rgba) = node.attribute("rgba") {
            let parts: Vec<f32> = rgba.split_whitespace()
                .filter_map(|s| s.parse::<f32>().ok())
                .collect();
            if parts.len() == 4 && parts.iter().all(|v| (0.0..=1.0).contains(v)) {
                out.push(ColorInformation {
                    range: crate::document::attr_value_range(text, &node, "rgba"),
                    color: Color { red: parts[0], green: parts[1], blue: parts[2], alpha: parts[3] },
                });
            }
        }
    }
    for child in node.children() {
        walk_colors(child, text, out);
    }
}

/// When the user picks a new color in the picker, VS Code asks how to present
/// it. We return one option: the space-separated `r g b a` literal that
/// replaces the existing attribute value.
pub fn color_presentations(color: Color) -> Vec<ColorPresentation> {
    let fmt = |v: f32| {
        let s = format!("{v:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    vec![ColorPresentation {
        label: format!("{} {} {} {}", fmt(color.red), fmt(color.green), fmt(color.blue), fmt(color.alpha)),
        text_edit: None,
        additional_text_edits: None,
    }]
}

// ---------------------------------------------------------------------------
// 9. folding ranges
// ---------------------------------------------------------------------------

/// One folding range per multi-line XML element. Closing tag stays visible.
pub fn folding_ranges(text: &str) -> Vec<FoldingRange> {
    let Ok(xml) = roxmltree::Document::parse(text) else { return Vec::new(); };
    let mut out = Vec::new();
    walk_folding(xml.root_element(), text, &mut out);
    out
}

fn walk_folding(node: roxmltree::Node, text: &str, out: &mut Vec<FoldingRange>) {
    if node.is_element() {
        let r = node.range();
        let start_line = crate::document::byte_offset_to_position(text, r.start).line;
        let end_line = crate::document::byte_offset_to_position(text, r.end).line;
        // Only emit when there's at least one line of content between the
        // opening and closing tags; fold end is one above the closing tag so
        // the closing tag remains visible when collapsed.
        if end_line > start_line + 1 {
            out.push(FoldingRange {
                start_line,
                start_character: None,
                end_line: end_line - 1,
                end_character: None,
                kind: None,
                collapsed_text: None,
            });
        }
    }
    for child in node.children() {
        walk_folding(child, text, out);
    }
}

// ---------------------------------------------------------------------------
// 9. code actions / quick fixes
// ---------------------------------------------------------------------------

/// Produce quick-fix CodeActions for each diagnostic in `diagnostics` that we
/// know how to repair (typo corrections + insert missing required attribute).
pub fn code_actions(
    doc: &Document,
    text: &str,
    diagnostics: &[Diagnostic],
    uri: &Url,
) -> CodeActionResponse {
    let mut out: CodeActionResponse = Vec::new();
    for diag in diagnostics {
        suggest_for(doc, text, diag, uri, &mut out);
    }
    out
}

fn suggest_for(
    doc: &Document,
    text: &str,
    diag: &Diagnostic,
    uri: &Url,
    out: &mut CodeActionResponse,
) {
    let msg = diag.message.as_str();

    if let Some(bad) = between(msg, "Undefined link '", "'") {
        for cand in similar(bad, doc.links.iter().map(|l| l.name.as_str())) {
            out.push(replace_range_action(uri, diag, &cand, format!("Change to '{cand}'")));
        }
    } else if let Some(bad) = between(msg, "Undefined joint '", "' in mimic") {
        for cand in similar(bad, doc.joints.iter().map(|j| j.name.as_str())) {
            out.push(replace_range_action(uri, diag, &cand, format!("Change to '{cand}'")));
        }
    } else if let Some(bad) = between(msg, "Undefined link or joint '", "' in gazebo reference") {
        let pool = doc.links.iter().map(|l| l.name.as_str())
            .chain(doc.joints.iter().map(|j| j.name.as_str()));
        for cand in similar(bad, pool) {
            out.push(replace_range_action(uri, diag, &cand, format!("Change to '{cand}'")));
        }
    } else if let Some(bad) = between(msg, "Undefined xacro property '", "'") {
        for cand in similar(bad, doc.xacro_properties.iter().map(|p| p.name.as_str())) {
            // The diag range covers ${badname} including braces — wrap candidate the same way
            out.push(replace_range_action(
                uri,
                diag,
                &format!("${{{cand}}}"),
                format!("Change to '${{{cand}}}'"),
            ));
        }
    } else if msg.contains("is missing required attribute") {
        if let Some(attr) = between(msg, "is missing required attribute '", "'") {
            if let Some(action) = insert_attribute_action(uri, diag, text, attr) {
                out.push(action);
            }
        }
    }
}

/// Extract the substring of `s` between `prefix` and the next `suffix`.
fn between<'a>(s: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    let start = s.find(prefix)? + prefix.len();
    let rest = &s[start..];
    let end = rest.find(suffix)?;
    Some(&rest[..end])
}

/// Up to 3 candidates from `pool` within edit distance roughly `len/3` (min 1).
fn similar<'a>(target: &str, pool: impl Iterator<Item = &'a str>) -> Vec<String> {
    let max_dist = (target.chars().count() / 3).max(1);
    let mut scored: Vec<(String, usize)> = pool
        .filter_map(|c| {
            if c == target { return None; }
            let d = edit_distance(target, c);
            if d <= max_dist { Some((c.to_string(), d)) } else { None }
        })
        .collect();
    scored.sort_by_key(|(_, d)| *d);
    scored.into_iter().take(3).map(|(s, _)| s).collect()
}

/// Standard DP Levenshtein, byte-wise (fine for ASCII identifier names).
fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.is_empty() { return b.len(); }
    if b.is_empty() { return a.len(); }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0; b.len() + 1];
    for (i, &ai) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &bj) in b.iter().enumerate() {
            let cost = if ai == bj { 0 } else { 1 };
            curr[j + 1] = (curr[j] + 1)
                .min(prev[j + 1] + 1)
                .min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

fn replace_range_action(uri: &Url, diag: &Diagnostic, new_text: &str, title: String) -> CodeActionOrCommand {
    let mut changes = std::collections::HashMap::new();
    changes.insert(uri.clone(), vec![TextEdit { range: diag.range, new_text: new_text.to_string() }]);
    CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diag.clone()]),
        edit: Some(WorkspaceEdit { changes: Some(changes), ..WorkspaceEdit::default() }),
        is_preferred: Some(true),
        ..CodeAction::default()
    })
}

/// Build an action that inserts `attr=""` into the opening tag at `diag.range`.
/// The diagnostic's range covers the tag name (e.g. `link` in `<link ...>`); we
/// insert the new attribute right after it, before any existing attributes.
fn insert_attribute_action(uri: &Url, diag: &Diagnostic, text: &str, attr: &str) -> Option<CodeActionOrCommand> {
    let insert_pos = Position {
        line: diag.range.end.line,
        character: diag.range.end.character,
    };
    // Sanity check: the character at the diag end should be inside an opening tag
    // (next non-whitespace before some attribute or `>`/`/>`).
    let line = text.lines().nth(diag.range.end.line as usize)?;
    let after = line.get(diag.range.end.character as usize..)?;
    if !after.starts_with(|c: char| c == ' ' || c == '\t' || c == '>' || c == '/' || c == '\n') {
        return None;
    }
    let edit = TextEdit {
        range: Range::new(insert_pos, insert_pos),
        new_text: format!(" {attr}=\"\""),
    };
    let mut changes = std::collections::HashMap::new();
    changes.insert(uri.clone(), vec![edit]);
    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Add missing attribute {attr}=\"\""),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diag.clone()]),
        edit: Some(WorkspaceEdit { changes: Some(changes), ..WorkspaceEdit::default() }),
        is_preferred: Some(true),
        ..CodeAction::default()
    }))
}

fn position_in_range(pos: Position, range: Range) -> bool {
    let after_start = pos.line > range.start.line
        || (pos.line == range.start.line && pos.character >= range.start.character);
    let before_end = pos.line < range.end.line
        || (pos.line == range.end.line && pos.character <= range.end.character);
    after_start && before_end
}
