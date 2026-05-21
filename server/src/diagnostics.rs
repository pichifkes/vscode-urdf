use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Range};
use crate::document::Document;

pub fn check(doc: &Document, text: &str) -> Vec<Diagnostic> {
    let mut diags: Vec<Diagnostic> = Vec::new();

    let link_names: std::collections::HashSet<&str> =
        doc.links.iter().map(|l| l.name.as_str()).collect();
    let joint_names: std::collections::HashSet<&str> =
        doc.joints.iter().map(|j| j.name.as_str()).collect();

    // 1 & 2. Undefined link references in joints (parent and child)
    for joint in &doc.joints {
        if let Some(parent_ref) = &joint.parent {
            if !link_names.contains(parent_ref.name.as_str()) {
                diags.push(make_diag(
                    parent_ref.range,
                    DiagnosticSeverity::ERROR,
                    format!("Undefined link '{}'", parent_ref.name),
                ));
            }
        }
        if let Some(child_ref) = &joint.child {
            if !link_names.contains(child_ref.name.as_str()) {
                diags.push(make_diag(
                    child_ref.range,
                    DiagnosticSeverity::ERROR,
                    format!("Undefined link '{}'", child_ref.name),
                ));
            }
        }

        // 3. Undefined joint mimic reference
        if let Some(mimic_ref) = &joint.mimic {
            if !joint_names.contains(mimic_ref.name.as_str()) {
                diags.push(make_diag(
                    mimic_ref.range,
                    DiagnosticSeverity::ERROR,
                    format!("Undefined joint '{}' in mimic", mimic_ref.name),
                ));
            }
        }

        // 6. Self-referential joint
        if let (Some(p), Some(c)) = (&joint.parent, &joint.child) {
            if p.name == c.name {
                diags.push(make_diag(
                    joint.range,
                    DiagnosticSeverity::ERROR,
                    format!("Joint '{}' has the same link as parent and child", joint.name),
                ));
            }
        }
    }

    // 4. Duplicate link names
    {
        let mut seen: std::collections::HashMap<&str, bool> = std::collections::HashMap::new();
        for item in &doc.links {
            if seen.contains_key(item.name.as_str()) {
                diags.push(make_diag(
                    item.range,
                    DiagnosticSeverity::ERROR,
                    format!("Duplicate link name '{}'", item.name),
                ));
            } else {
                seen.insert(item.name.as_str(), true);
            }
        }
    }

    // 5. Duplicate joint names
    {
        let mut seen: std::collections::HashMap<&str, bool> = std::collections::HashMap::new();
        for item in &doc.joints {
            if seen.contains_key(item.name.as_str()) {
                diags.push(make_diag(
                    item.range,
                    DiagnosticSeverity::ERROR,
                    format!("Duplicate joint name '{}'", item.name),
                ));
            } else {
                seen.insert(item.name.as_str(), true);
            }
        }
    }

    // 7. Undefined xacro property references: scan text for ${varname}
    {
        let prop_names: std::collections::HashSet<&str> =
            doc.xacro_properties.iter().map(|p| p.name.as_str()).collect();

        let bytes = text.as_bytes();
        let mut i = 0;
        while i + 1 < bytes.len() {
            if bytes[i] == b'$' && bytes[i + 1] == b'{' {
                let start = i;
                let inner_start = i + 2;
                // Find closing '}'
                if let Some(rel) = bytes[inner_start..].iter().position(|&b| b == b'}') {
                    let inner_end = inner_start + rel;
                    let varname = &text[inner_start..inner_end];
                    // Skip empty or varnames with spaces
                    if !varname.is_empty() && !varname.contains(' ') {
                        if !prop_names.contains(varname) {
                            let end = inner_end + 1; // past '}'
                            let range = byte_range_to_lsp(text, start..end);
                            diags.push(make_diag(
                                range,
                                DiagnosticSeverity::ERROR,
                                format!("Undefined xacro property '{}'", varname),
                            ));
                        }
                    }
                    i = inner_end + 1;
                    continue;
                }
            }
            i += 1;
        }
    }

    diags
}

fn make_diag(range: Range, severity: DiagnosticSeverity, message: String) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(severity),
        source: Some("urdf-lsp".to_string()),
        message,
        ..Diagnostic::default()
    }
}

fn byte_offset_to_position(text: &str, offset: usize) -> tower_lsp::lsp_types::Position {
    let safe_offset = offset.min(text.len());
    let before = &text[..safe_offset];
    let line = before.bytes().filter(|&b| b == b'\n').count();
    let last_newline = before.rfind('\n').map(|p| p + 1).unwrap_or(0);
    let character = before[last_newline..].chars().count();
    tower_lsp::lsp_types::Position {
        line: line as u32,
        character: character as u32,
    }
}

fn byte_range_to_lsp(text: &str, range: std::ops::Range<usize>) -> Range {
    Range {
        start: byte_offset_to_position(text, range.start),
        end: byte_offset_to_position(text, range.end),
    }
}
