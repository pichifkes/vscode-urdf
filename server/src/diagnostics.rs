use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Range};
use crate::document::Document;

pub fn check(doc: &Document, text: &str) -> Vec<Diagnostic> {
    let mut diags: Vec<Diagnostic> = Vec::new();

    let link_names: std::collections::HashSet<&str> =
        doc.links.iter().map(|l| l.name.as_str()).collect();
    let joint_names: std::collections::HashSet<&str> =
        doc.joints.iter().map(|j| j.name.as_str()).collect();

    // In xacro files, links/joints may come from included files — use Warning
    let undef_severity = if doc.is_xacro {
        DiagnosticSeverity::WARNING
    } else {
        DiagnosticSeverity::ERROR
    };

    // 1 & 2. Undefined link references in joints (parent and child)
    for joint in &doc.joints {
        if let Some(parent_ref) = &joint.parent {
            if !link_names.contains(parent_ref.name.as_str()) {
                diags.push(make_diag(
                    parent_ref.range,
                    undef_severity,
                    format!("Undefined link '{}'", parent_ref.name),
                ));
            }
        }
        if let Some(child_ref) = &joint.child {
            if !link_names.contains(child_ref.name.as_str()) {
                diags.push(make_diag(
                    child_ref.range,
                    undef_severity,
                    format!("Undefined link '{}'", child_ref.name),
                ));
            }
        }

        // 3. Undefined joint mimic reference
        if let Some(mimic_ref) = &joint.mimic {
            if !joint_names.contains(mimic_ref.name.as_str()) {
                diags.push(make_diag(
                    mimic_ref.range,
                    undef_severity,
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
                    if !varname.is_empty()
                        && !varname.contains(|c: char| matches!(c, ' ' | '+' | '-' | '*' | '/' | '(' | ')' | '.'))
                    {
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

// ---------------------------------------------------------------------------
// Schema validation: unknown element names and unknown attributes
// ---------------------------------------------------------------------------

pub fn check_schema(text: &str) -> Vec<Diagnostic> {
    let xml = match roxmltree::Document::parse(text) {
        Ok(d) => d,
        Err(_) => return vec![], // XML errors already reported by document::parse
    };

    // Collect xacro property values so simple ${varname} can be type-checked
    let mut props: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for node in xml.root_element().children() {
        if node.is_element() {
            let tag = node.tag_name();
            if tag.name() == "property" && tag.namespace().map_or(false, |n| n.contains("xacro")) {
                if let (Some(name), Some(value)) = (node.attribute("name"), node.attribute("value")) {
                    props.insert(name, value);
                }
            }
        }
    }

    let mut diags = vec![];
    walk_schema(xml.root_element(), text, &mut diags, false, &props);
    diags
}

fn walk_schema(
    node: roxmltree::Node,
    text: &str,
    diags: &mut Vec<Diagnostic>,
    skip: bool,
    props: &std::collections::HashMap<&str, &str>,
) {
    if node.is_text() {
        if !skip {
            let content = node.text().unwrap_or("").trim();
            if !content.is_empty() {
                let range = byte_range_to_lsp(text, node.range());
                diags.push(make_diag(
                    range,
                    DiagnosticSeverity::WARNING,
                    "Unexpected text content in URDF element".to_string(),
                ));
            }
        }
        return;
    }
    if !node.is_element() {
        return;
    }

    let tag = node.tag_name().name();
    let ns = node.tag_name().namespace();

    // xacro namespace elements are always valid
    let is_xacro = ns.map_or(false, |n| n.contains("xacro")) || tag.starts_with("xacro:");

    // Don't validate inside gazebo/plugin/transmission — they accept arbitrary XML
    let child_skip = skip || is_xacro || matches!(tag, "gazebo" | "plugin" | "sensor" | "transmission");

    if !skip && !is_xacro {
        match known_urdf_attrs(tag) {
            Some(valid_attrs) => {
                for attr in node.attributes() {
                    let aname = attr.name();
                    // skip XML namespace declarations
                    if aname == "xmlns" || aname.starts_with("xmlns:") {
                        continue;
                    }
                    if !valid_attrs.contains(&aname) {
                        let range = attr_name_range(text, &node, aname);
                        diags.push(make_diag(
                            range,
                            DiagnosticSeverity::WARNING,
                            format!("Unknown attribute '{aname}' on element <{tag}>"),
                        ));
                    }
                }

                // Check 1: required attributes
                for req in required_urdf_attrs(tag) {
                    if node.attribute(*req).is_none() {
                        let range = elem_name_range(text, &node);
                        diags.push(make_diag(
                            range,
                            DiagnosticSeverity::ERROR,
                            format!("Element <{tag}> is missing required attribute '{req}'"),
                        ));
                    }
                }

                // Check 2: attribute value types
                for attr in node.attributes() {
                    let aname = attr.name();
                    if aname == "xmlns" || aname.starts_with("xmlns:") {
                        continue;
                    }
                    if let Some(msg) = validate_attr_value(tag, aname, attr.value(), props) {
                        let range = attr_name_range(text, &node, aname);
                        diags.push(make_diag(range, DiagnosticSeverity::ERROR, msg));
                    }
                }
            }
            None => {
                let range = elem_name_range(text, &node);
                diags.push(make_diag(
                    range,
                    DiagnosticSeverity::WARNING,
                    format!("Unknown URDF element <{tag}>"),
                ));
            }
        }
    }

    for child in node.children() {
        walk_schema(child, text, diags, child_skip, props);
    }
}

fn required_urdf_attrs(element: &str) -> &'static [&'static str] {
    match element {
        "robot"    => &[],   // name is optional in xacro fragments
        "link"     => &["name"],
        "joint"    => &["name", "type"],
        "parent"   => &["link"],
        "child"    => &["link"],
        "box"      => &["size"],
        "cylinder" => &["radius", "length"],
        "sphere"   => &["radius"],
        "mesh"     => &["filename"],
        "color"    => &["rgba"],
        "mass"     => &["value"],
        "mimic"    => &["joint"],
        _          => &[],
    }
}

fn validate_attr_value(
    element: &str,
    attr: &str,
    value: &str,
    props: &std::collections::HashMap<&str, &str>,
) -> Option<String> {
    // If the value contains xacro substitutions, try to resolve a simple
    // ${varname} to its property value. If resolution fails or the expression
    // is complex (operators, multiple vars), skip type checking entirely.
    let resolved;
    let effective: &str = if value.contains("${") {
        if let Some(v) = resolve_simple_xacro(value, props) {
            resolved = v;
            &resolved
        } else {
            return None;
        }
    } else {
        value
    };
    match (element, attr) {
        // 3 floats
        (_, "xyz") | (_, "rpy") => expect_n_floats(effective, 3, attr),
        // 3 positive floats
        ("box", "size") => expect_n_floats(effective, 3, attr)
            .or_else(|| {
                let ok = effective.split_whitespace()
                    .filter_map(|s| s.parse::<f64>().ok())
                    .all(|f| f > 0.0);
                if !ok { Some("'size' values must be positive".to_string()) } else { None }
            }),
        // 4 floats in [0, 1]
        ("color", "rgba") => expect_n_floats(effective, 4, attr)
            .or_else(|| {
                let vals: Vec<f64> = effective.split_whitespace()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                if vals.len() == 4 && vals.iter().any(|&f| f < 0.0 || f > 1.0) {
                    Some("'rgba' values must be between 0 and 1".into())
                } else { None }
            }),
        // single positive float
        (_, "radius") | ("cylinder", "length") => expect_positive_float(effective, attr),
        // single float (can be negative)
        (_, "lower") | (_, "upper") | (_, "effort") | (_, "velocity")
        | (_, "damping") | (_, "friction") | (_, "value")
        | (_, "ixx") | (_, "ixy") | (_, "ixz") | (_, "iyy") | (_, "iyz") | (_, "izz")
        | (_, "multiplier") | (_, "offset") => expect_float(effective, attr),
        _ => None,
    }
}

/// Resolve `${varname}` to its property value when the substitution is a single
/// simple identifier (no operators, spaces, or parentheses). Returns None for
/// complex expressions or unresolvable names — caller should skip type checking.
fn resolve_simple_xacro<'a>(
    value: &str,
    props: &'a std::collections::HashMap<&str, &str>,
) -> Option<String> {
    let v = value.trim();
    // Must be exactly ${varname} with nothing else
    if !v.starts_with("${") || !v.ends_with('}') {
        return None;
    }
    let inner = &v[2..v.len() - 1];
    // Reject expressions — anything with operators or whitespace
    if inner.contains(|c: char| matches!(c, '+' | '-' | '*' | '/' | '(' | ')' | ' ' | '\t')) {
        return None;
    }
    let resolved = props.get(inner)?;
    // Don't recurse into nested xacro expressions
    if resolved.contains("${") {
        return None;
    }
    Some(resolved.to_string())
}

fn expect_n_floats(value: &str, n: usize, attr: &str) -> Option<String> {
    let parts: Vec<&str> = value.split_whitespace().collect();
    if parts.len() != n {
        return Some(format!("'{attr}' expects {n} numbers, got {}", parts.len()));
    }
    let bad: Vec<&str> = parts.iter().copied()
        .filter(|s| s.parse::<f64>().is_err())
        .collect();
    if !bad.is_empty() {
        return Some(format!("'{attr}' contains non-numeric value: '{}'", bad[0]));
    }
    None
}

fn expect_float(value: &str, attr: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.parse::<f64>().is_err() {
        Some(format!("'{attr}' must be a number, got '{trimmed}'"))
    } else { None }
}

fn expect_positive_float(value: &str, attr: &str) -> Option<String> {
    let trimmed = value.trim();
    match trimmed.parse::<f64>() {
        Ok(f) if f > 0.0 => None,
        Ok(_) => Some(format!("'{attr}' must be positive")),
        Err(_) => Some(format!("'{attr}' must be a number, got '{trimmed}'")),
    }
}

fn known_urdf_attrs(element: &str) -> Option<&'static [&'static str]> {
    match element {
        "robot"             => Some(&["name"]),
        "link"              => Some(&["name"]),
        "joint"             => Some(&["name", "type"]),
        "visual"            => Some(&["name"]),
        "collision"         => Some(&["name"]),
        "inertial"          => Some(&[]),
        "origin"            => Some(&["xyz", "rpy"]),
        "geometry"          => Some(&[]),
        "box"               => Some(&["size"]),
        "cylinder"          => Some(&["radius", "length"]),
        "sphere"            => Some(&["radius"]),
        "mesh"              => Some(&["filename", "scale"]),
        "material"          => Some(&["name"]),
        "color"             => Some(&["rgba"]),
        "texture"           => Some(&["filename"]),
        "mass"              => Some(&["value"]),
        "inertia"           => Some(&["ixx", "ixy", "ixz", "iyy", "iyz", "izz"]),
        "parent"            => Some(&["link"]),
        "child"             => Some(&["link"]),
        "axis"              => Some(&["xyz"]),
        "limit"             => Some(&["lower", "upper", "effort", "velocity"]),
        "dynamics"          => Some(&["damping", "friction"]),
        "safety_controller" => Some(&["soft_lower_limit", "soft_upper_limit", "k_position", "k_velocity"]),
        "calibration"       => Some(&["rising", "falling"]),
        "mimic"             => Some(&["joint", "multiplier", "offset"]),
        "transmission"      => Some(&["name"]),
        "gazebo"            => Some(&["reference"]),
        _                   => None,
    }
}

/// Range covering the tag name in the opening tag (e.g. `bosx` in `<bosx ...>`).
fn elem_name_range(text: &str, node: &roxmltree::Node) -> Range {
    let start = node.range().start + 1; // skip '<'
    let name = node.tag_name().name();
    byte_range_to_lsp(text, start..start + name.len())
}

/// Range covering the attribute name within the element source.
fn attr_name_range(text: &str, node: &roxmltree::Node, attr_name: &str) -> Range {
    let elem_range = node.range();
    let elem_src = &text[elem_range.clone()];
    let mut search = 0;
    while search < elem_src.len() {
        let Some(rel) = elem_src[search..].find(attr_name) else {
            break;
        };
        let abs = search + rel;
        let prev_ok = abs == 0 || elem_src.as_bytes()[abs - 1].is_ascii_whitespace();
        let after = abs + attr_name.len();
        let next_ok = after < elem_src.len()
            && matches!(elem_src.as_bytes()[after], b'=' | b' ' | b'\t' | b'\n' | b'\r');
        if prev_ok && next_ok {
            let start = elem_range.start + abs;
            return byte_range_to_lsp(text, start..start + attr_name.len());
        }
        search = abs + 1;
    }
    elem_name_range(text, node) // fallback
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
