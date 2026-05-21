use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

#[derive(Debug, Clone)]
pub struct Document {
    pub links: Vec<NamedItem>,
    pub joints: Vec<Joint>,
    pub materials: Vec<NamedItem>,
    pub xacro_properties: Vec<NamedItem>,
    /// `reference` attribute values from `<gazebo reference="...">` elements.
    pub gazebo_refs: Vec<NameRef>,
    /// True when the root element declares xmlns:xacro — indicates a xacro fragment
    /// where some links/joints may be defined in included files.
    pub is_xacro: bool,
}

#[derive(Debug, Clone)]
pub struct NamedItem {
    pub name: String,
    pub range: Range,
}

#[derive(Debug, Clone)]
pub struct Joint {
    pub name: String,
    pub range: Range,
    pub joint_type: Option<String>,
    pub parent: Option<NameRef>,
    pub child: Option<NameRef>,
    pub mimic: Option<NameRef>,
}

#[derive(Debug, Clone)]
pub struct NameRef {
    pub name: String,
    pub range: Range,
}

pub fn parse(text: &str) -> (Document, Vec<Diagnostic>) {
    let mut doc = Document {
        links: Vec::new(),
        joints: Vec::new(),
        materials: Vec::new(),
        xacro_properties: Vec::new(),
        gazebo_refs: Vec::new(),
        is_xacro: false,
    };

    let xml = match roxmltree::Document::parse(text) {
        Ok(d) => d,
        Err(e) => {
            // Tag-balance scanner: when XML parsing fails, try to identify
            // mismatched/unclosed tags ourselves so the diagnostic points at
            // the actual misspelled opening tag, not the closing tag where
            // roxmltree first noticed the inconsistency.
            let balance_diags = scan_tag_balance(text);
            if !balance_diags.is_empty() {
                return (doc, balance_diags);
            }
            let msg = e.to_string();
            let (line, col) = parse_xml_error_pos(&msg);
            let pos = Position::new(line, col);
            let diag = Diagnostic {
                range: Range::new(pos, Position::new(line, col + 1)),
                severity: Some(DiagnosticSeverity::ERROR),
                source: Some("urdf-lsp".into()),
                message: format!("XML parse error: {e}"),
                ..Diagnostic::default()
            };
            return (doc, vec![diag]);
        }
    };

    let root = xml.root_element();

    doc.is_xacro = root.attributes().any(|a| {
        (a.name() == "xmlns:xacro" || a.name().starts_with("xmlns:"))
            && a.value().contains("xacro")
    });

    for node in root.children() {
        if !node.is_element() {
            continue;
        }

        let tag = node.tag_name().name();

        match tag {
            "link" => {
                if let Some(name) = node.attribute("name") {
                    let range = attr_value_range(text, &node, "name");
                    doc.links.push(NamedItem {
                        name: name.to_string(),
                        range,
                    });
                }
            }
            "joint" => {
                if let Some(name) = node.attribute("name") {
                    let range = attr_value_range(text, &node, "name");
                    let joint_type = node.attribute("type").map(|s| s.to_string());

                    let mut parent: Option<NameRef> = None;
                    let mut child: Option<NameRef> = None;
                    let mut mimic: Option<NameRef> = None;

                    for child_node in node.children() {
                        if !child_node.is_element() {
                            continue;
                        }
                        match child_node.tag_name().name() {
                            "parent" => {
                                if let Some(link_name) = child_node.attribute("link") {
                                    let r = attr_value_range(text, &child_node, "link");
                                    parent = Some(NameRef {
                                        name: link_name.to_string(),
                                        range: r,
                                    });
                                }
                            }
                            "child" => {
                                if let Some(link_name) = child_node.attribute("link") {
                                    let r = attr_value_range(text, &child_node, "link");
                                    child = Some(NameRef {
                                        name: link_name.to_string(),
                                        range: r,
                                    });
                                }
                            }
                            "mimic" => {
                                if let Some(joint_name) = child_node.attribute("joint") {
                                    let r = attr_value_range(text, &child_node, "joint");
                                    mimic = Some(NameRef {
                                        name: joint_name.to_string(),
                                        range: r,
                                    });
                                }
                            }
                            _ => {}
                        }
                    }

                    doc.joints.push(Joint {
                        name: name.to_string(),
                        range,
                        joint_type,
                        parent,
                        child,
                        mimic,
                    });
                }
            }
            "material" => {
                // Only top-level material definitions (those with a name attribute at robot level)
                if let Some(name) = node.attribute("name") {
                    let range = attr_value_range(text, &node, "name");
                    doc.materials.push(NamedItem {
                        name: name.to_string(),
                        range,
                    });
                }
            }
            "gazebo" => {
                if let Some(reference) = node.attribute("reference") {
                    let range = attr_value_range(text, &node, "reference");
                    doc.gazebo_refs.push(NameRef {
                        name: reference.to_string(),
                        range,
                    });
                }
            }
            _ => {
                // xacro:property — tag name includes the namespace prefix in roxmltree
                // The tag_name().name() strips the namespace, so we check the full name
                let full_name = node.tag_name();
                if full_name.name() == "property" && full_name.namespace() == Some("http://www.ros.org/wiki/xacro") {
                    if let Some(name) = node.attribute("name") {
                        let range = attr_value_range(text, &node, "name");
                        doc.xacro_properties.push(NamedItem {
                            name: name.to_string(),
                            range,
                        });
                    }
                } else if tag == "xacro:property" {
                    // Fallback: some parsers present it as "xacro:property" in the name
                    if let Some(name) = node.attribute("name") {
                        let range = attr_value_range(text, &node, "name");
                        doc.xacro_properties.push(NamedItem {
                            name: name.to_string(),
                            range,
                        });
                    }
                }
            }
        }
    }

    (doc, vec![])
}

/// Parse a row:col prefix from roxmltree error messages (e.g. "9:5 unexpected close tag").
/// Returns 0-indexed (line, character). Falls back to (0, 0) if the format doesn't match.
/// Walk the document tracking opening/closing tag balance. Returns a single
/// diagnostic positioned on the actual misspelled (or unclosed) opening tag,
/// rather than at the closing tag where the inconsistency was detected.
/// Used as a fallback when roxmltree::Document::parse fails.
fn scan_tag_balance(text: &str) -> Vec<Diagnostic> {
    let bytes = text.as_bytes();
    // Stack entries: (tag name, byte start of name, byte end of name)
    let mut stack: Vec<(&str, usize, usize)> = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        i += 1;
        if i >= bytes.len() {
            break;
        }

        // <?xml … ?>  or  <?target …?>
        if bytes[i] == b'?' {
            i += 1;
            while i + 1 < bytes.len() && !(bytes[i] == b'?' && bytes[i + 1] == b'>') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }

        if bytes[i] == b'!' {
            // Comment <!-- … -->
            if i + 2 < bytes.len() && bytes[i + 1] == b'-' && bytes[i + 2] == b'-' {
                i += 3;
                while i + 2 < bytes.len()
                    && !(bytes[i] == b'-' && bytes[i + 1] == b'-' && bytes[i + 2] == b'>')
                {
                    i += 1;
                }
                i = (i + 3).min(bytes.len());
                continue;
            }
            // CDATA <![CDATA[ … ]]>
            if i + 7 < bytes.len() && &bytes[i + 1..i + 8] == b"[CDATA[" {
                i += 8;
                while i + 2 < bytes.len()
                    && !(bytes[i] == b']' && bytes[i + 1] == b']' && bytes[i + 2] == b'>')
                {
                    i += 1;
                }
                i = (i + 3).min(bytes.len());
                continue;
            }
            // DOCTYPE or any other declaration — skip to '>'
            while i < bytes.len() && bytes[i] != b'>' {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
            continue;
        }

        // Closing tag </name>
        if bytes[i] == b'/' {
            i += 1;
            let name_start = i;
            while i < bytes.len() && is_name_char(bytes[i]) {
                i += 1;
            }
            let name_end = i;
            let name = &text[name_start..name_end];
            while i < bytes.len() && bytes[i] != b'>' {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }

            match stack.pop() {
                Some((open_name, _, _)) if open_name == name => {}
                Some((open_name, open_ns, open_ne)) => {
                    return vec![diag_at(
                        text,
                        open_ns..open_ne,
                        format!(
                            "Mismatched tag: opened <{open_name}> but closed with </{name}>"
                        ),
                    )];
                }
                None => {
                    return vec![diag_at(
                        text,
                        name_start..name_end,
                        format!("Closing tag </{name}> has no matching opening tag"),
                    )];
                }
            }
            continue;
        }

        // Opening tag <name …>  or  <name … />
        let name_start = i;
        while i < bytes.len() && is_name_char(bytes[i]) {
            i += 1;
        }
        let name_end = i;
        let name = &text[name_start..name_end];
        if name.is_empty() {
            continue;
        }

        // Walk to '>' or '/>', honouring quoted attribute values.
        let mut self_closing = false;
        let mut in_quote: Option<u8> = None;
        while i < bytes.len() {
            let b = bytes[i];
            match in_quote {
                Some(q) if b == q => in_quote = None,
                Some(_) => {}
                None => {
                    if b == b'"' || b == b'\'' {
                        in_quote = Some(b);
                    } else if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'>' {
                        self_closing = true;
                        i += 2;
                        break;
                    } else if b == b'>' {
                        i += 1;
                        break;
                    }
                }
            }
            i += 1;
        }

        if !self_closing {
            stack.push((name, name_start, name_end));
        }
    }

    // Anything left on the stack is an unclosed tag.
    if let Some(&(open_name, open_ns, open_ne)) = stack.last() {
        return vec![diag_at(
            text,
            open_ns..open_ne,
            format!("Tag <{open_name}> is never closed"),
        )];
    }

    Vec::new()
}

fn is_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b':' | b'-' | b'.')
}

fn diag_at(text: &str, range: std::ops::Range<usize>, msg: String) -> Diagnostic {
    Diagnostic {
        range: byte_range_to_lsp(text, range),
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("urdf-lsp".into()),
        message: msg,
        ..Diagnostic::default()
    }
}

fn parse_xml_error_pos(msg: &str) -> (u32, u32) {
    let mut iter = msg.splitn(3, ':');
    let row = iter.next().and_then(|s| s.trim().parse::<u32>().ok());
    let rest = iter.next().unwrap_or("");
    let col = rest.split_whitespace().next().and_then(|s| s.parse::<u32>().ok());
    match (row, col) {
        (Some(r), Some(c)) if r > 0 => (r - 1, c.saturating_sub(1)),
        _ => (0, 0),
    }
}

/// Get the LSP Range for the value of a named attribute on the given node.
/// Searches for the attribute value inside the element's source span.
/// Falls back to the node's own range if the value can't be located.
fn attr_value_range(text: &str, node: &roxmltree::Node, attr_name: &str) -> Range {
    let value = match node.attribute(attr_name) {
        Some(v) => v,
        None => {
            let span = node.range();
            return byte_range_to_lsp(text, span);
        }
    };

    // The element's byte range in the source
    let elem_range = node.range();
    let elem_src = &text[elem_range.clone()];

    // Look for attr_name="value" or attr_name='value' within the element source.
    // We search for the attribute name followed by = and then the quoted value.
    let needle_dq = format!("{}=\"{}\"", attr_name, value);
    let needle_sq = format!("{}='{}'", attr_name, value);

    let offset = elem_src
        .find(needle_dq.as_str())
        .map(|pos| pos + attr_name.len() + 2) // skip name="
        .or_else(|| {
            elem_src
                .find(needle_sq.as_str())
                .map(|pos| pos + attr_name.len() + 2) // skip name='
        });

    if let Some(rel_start) = offset {
        let start = elem_range.start + rel_start;
        let end = start + value.len();
        byte_range_to_lsp(text, start..end)
    } else {
        // Fallback: use the element's span
        byte_range_to_lsp(text, elem_range)
    }
}

pub(crate) fn byte_offset_to_position(text: &str, offset: usize) -> Position {
    let safe_offset = offset.min(text.len());
    let before = &text[..safe_offset];
    let line = before.bytes().filter(|&b| b == b'\n').count();
    let last_newline = before.rfind('\n').map(|p| p + 1).unwrap_or(0);
    let character = before[last_newline..].chars().count();
    Position {
        line: line as u32,
        character: character as u32,
    }
}

pub(crate) fn byte_range_to_lsp(text: &str, range: std::ops::Range<usize>) -> Range {
    Range {
        start: byte_offset_to_position(text, range.start),
        end: byte_offset_to_position(text, range.end),
    }
}
