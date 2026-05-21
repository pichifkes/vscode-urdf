use std::borrow::Cow;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Range};
use crate::document::Document;

/// Canonical list of Gazebo property element names.
/// Shared with the completion provider — keep in sync with `known_gazebo_prop`.
pub const GAZEBO_PROP_NAMES: &[&str] = &[
    "mu1", "mu2", "mu", "kp", "kd",
    "maxVel", "minDepth", "maxContacts",
    "selfCollide", "turnGravityOff", "gravity", "implicitSpringDamper",
    "dampingFactor", "laserRetro", "material",
    "stopCfm", "stopErp", "fudgeFactor",
    "sensor", "plugin",
];

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

        // Self-referential joint
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
        let mut seen = std::collections::HashSet::new();
        for item in &doc.links {
            if !seen.insert(item.name.as_str()) {
                diags.push(make_diag(
                    item.range,
                    DiagnosticSeverity::ERROR,
                    format!("Duplicate link name '{}'", item.name),
                ));
            }
        }
    }

    // 5. Duplicate joint names
    {
        let mut seen = std::collections::HashSet::new();
        for item in &doc.joints {
            if !seen.insert(item.name.as_str()) {
                diags.push(make_diag(
                    item.range,
                    DiagnosticSeverity::ERROR,
                    format!("Duplicate joint name '{}'", item.name),
                ));
            }
        }
    }

    // 6. Kinematic tree: multiple roots, disconnected links, cycles
    if doc.links.len() > 1 {
        kinematic_tree_check(&doc, &mut diags, undef_severity);
    }

    // 7. Gazebo reference must point to a known link or joint
    for gref in &doc.gazebo_refs {
        if !link_names.contains(gref.name.as_str()) && !joint_names.contains(gref.name.as_str()) {
            diags.push(make_diag(
                gref.range,
                undef_severity,
                format!("Undefined link or joint '{}' in gazebo reference", gref.name),
            ));
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
                // Scan for closing '}', aborting at attribute/element boundaries so
                // an unclosed ${... doesn't silently swallow the rest of the file.
                let mut j = inner_start;
                let mut close: Option<usize> = None;
                while j < bytes.len() {
                    match bytes[j] {
                        b'}' => { close = Some(j); break; }
                        b'"' | b'\'' | b'<' | b'\n' => break,
                        _ => j += 1,
                    }
                }
                if let Some(inner_end) = close {
                    let varname = &text[inner_start..inner_end];
                    if !varname.is_empty()
                        && !varname.contains(|c: char| matches!(c, ' ' | '+' | '-' | '*' | '/' | '(' | ')' | '.'))
                    {
                        if !prop_names.contains(varname) {
                            let end = inner_end + 1;
                            let range = byte_range_to_lsp(text, start..end);
                            diags.push(make_diag(
                                range,
                                DiagnosticSeverity::ERROR,
                                format!("Undefined xacro property '{}'", varname),
                            ));
                        }
                    }
                    i = inner_end + 1;
                } else {
                    let range = byte_range_to_lsp(text, start..j);
                    diags.push(make_diag(
                        range,
                        DiagnosticSeverity::ERROR,
                        "Unclosed xacro expression: missing '}'".to_string(),
                    ));
                    i = j;
                }
                continue;
            }
            i += 1;
        }
    }

    diags
}

fn kinematic_tree_check(
    doc: &Document,
    diags: &mut Vec<Diagnostic>,
    severity: DiagnosticSeverity,
) {
    // Build parent→children adjacency and the set of all child link names.
    let mut adj: std::collections::HashMap<&str, Vec<&str>> = std::collections::HashMap::new();
    let mut child_set: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for joint in &doc.joints {
        if let (Some(p), Some(c)) = (&joint.parent, &joint.child) {
            adj.entry(p.name.as_str()).or_default().push(c.name.as_str());
            child_set.insert(c.name.as_str());
        }
    }

    // Isolated links: no parent joint AND no child joints — not part of any joint at all.
    for link in doc.links.iter()
        .filter(|l| !child_set.contains(l.name.as_str()) && !adj.contains_key(l.name.as_str()))
    {
        diags.push(make_diag(
            link.range,
            severity,
            format!("Link '{}' has no joints — not connected to the kinematic tree", link.name),
        ));
    }

    // Root links: no parent joint, but DO have children (i.e., appear as joint parents).
    let roots: Vec<&str> = doc.links.iter()
        .filter(|l| !child_set.contains(l.name.as_str()) && adj.contains_key(l.name.as_str()))
        .map(|l| l.name.as_str())
        .collect();

    // Multiple roots.
    if roots.len() > 1 {
        for link in doc.links.iter()
            .filter(|l| !child_set.contains(l.name.as_str()) && adj.contains_key(l.name.as_str()))
            .skip(1)
        {
            diags.push(make_diag(
                link.range,
                severity,
                format!(
                    "Link '{}' is a root (no parent joint); kinematic tree must have exactly one root",
                    link.name
                ),
            ));
        }
    }

    // DFS starting points = all links with no parent (roots + isolated).
    let all_roots: Vec<&str> = doc.links.iter()
        .filter(|l| !child_set.contains(l.name.as_str()))
        .map(|l| l.name.as_str())
        .collect();

    // Iterative DFS: detect cycles and collect reachable links.
    let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut on_path: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut stack: Vec<(&str, usize)> = Vec::new();

    for &root in &all_roots {
        if visited.contains(root) {
            continue;
        }
        stack.push((root, 0));
        on_path.insert(root);

        while !stack.is_empty() {
            let last = stack.len() - 1;
            let (node, idx) = stack[last];
            let kids: &[&str] = adj.get(node).map(|v| v.as_slice()).unwrap_or(&[]);

            if idx < kids.len() {
                stack[last].1 += 1;
                let kid = kids[idx];

                if on_path.contains(kid) {
                    // Back edge — report the joint that closes the cycle.
                    if let Some(joint) = doc.joints.iter().find(|j| {
                        j.parent.as_ref().map_or(false, |p| p.name == node)
                            && j.child.as_ref().map_or(false, |c| c.name == kid)
                    }) {
                        diags.push(make_diag(
                            joint.range,
                            DiagnosticSeverity::ERROR,
                            format!("Joint '{}' creates a cycle in the kinematic tree", joint.name),
                        ));
                    }
                } else if !visited.contains(kid) {
                    on_path.insert(kid);
                    stack.push((kid, 0));
                }
            } else {
                visited.insert(node);
                on_path.remove(node);
                stack.pop();
            }
        }
    }

    // Links not reachable from any root are disconnected.
    for link in &doc.links {
        if !visited.contains(link.name.as_str()) {
            diags.push(make_diag(
                link.range,
                severity,
                format!("Link '{}' is not connected to the kinematic tree", link.name),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Schema validation: unknown element names and unknown attributes
// ---------------------------------------------------------------------------

pub fn check_schema(xml: &roxmltree::Document, text: &str) -> Vec<Diagnostic> {
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
    let is_xacro = is_xacro_element(&node);
    let child_skip = skip || is_xacro || matches!(tag, "plugin" | "sensor" | "transmission");

    if !skip && !is_xacro {
        match known_urdf_attrs(tag) {
            Some(valid_attrs) => {
                for attr in node.attributes() {
                    let aname = attr.name();
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

    if tag == "gazebo" && !skip && !is_xacro {
        for child in node.children() {
            walk_gazebo_child(child, text, diags, props);
        }
    } else {
        for child in node.children() {
            walk_schema(child, text, diags, child_skip, props);
        }
    }
}

#[derive(Copy, Clone)]
enum GazeboPropKind {
    Float,
    NonNegFloat,
    PositiveFloat,
    Bool,
    Int,
    AnyString,
    Lenient,
}

fn known_gazebo_prop(tag: &str) -> Option<GazeboPropKind> {
    match tag {
        "mu1" | "mu2" | "mu"                          => Some(GazeboPropKind::NonNegFloat),
        "kp" | "kd"                                   => Some(GazeboPropKind::PositiveFloat),
        "maxVel" | "minDepth" | "stopCfm" | "stopErp" => Some(GazeboPropKind::NonNegFloat),
        "dampingFactor" | "laserRetro" | "fudgeFactor" => Some(GazeboPropKind::Float),
        "maxContacts"                                  => Some(GazeboPropKind::Int),
        "selfCollide" | "turnGravityOff" | "gravity"
        | "implicitSpringDamper"                       => Some(GazeboPropKind::Bool),
        "material"                                     => Some(GazeboPropKind::AnyString),
        "sensor" | "plugin"                            => Some(GazeboPropKind::Lenient),
        _                                              => None,
    }
}

fn walk_gazebo_child(
    node: roxmltree::Node,
    text: &str,
    diags: &mut Vec<Diagnostic>,
    props: &std::collections::HashMap<&str, &str>,
) {
    if !node.is_element() || is_xacro_element(&node) {
        return;
    }
    let tag = node.tag_name().name();
    match known_gazebo_prop(tag) {
        None => {
            let range = elem_name_range(text, &node);
            diags.push(make_diag(
                range,
                DiagnosticSeverity::WARNING,
                format!("Unknown Gazebo property <{tag}>"),
            ));
        }
        Some(GazeboPropKind::Lenient) => {}
        Some(kind) => {
            let mut text_content = String::new();
            let mut text_range: Option<std::ops::Range<usize>> = None;
            for child in node.children() {
                if child.is_text() {
                    let t = child.text().unwrap_or("");
                    if !t.trim().is_empty() {
                        text_content.push_str(t);
                        text_range = Some(child.range());
                    }
                }
            }
            let content = text_content.trim();
            if !content.is_empty() {
                if let Some(tr) = text_range {
                    if let Some(msg) = validate_gazebo_text(tag, content, kind, props) {
                        diags.push(make_diag(byte_range_to_lsp(text, tr), DiagnosticSeverity::ERROR, msg));
                    }
                }
            }
        }
    }
}

fn validate_gazebo_text(
    tag: &str,
    content: &str,
    kind: GazeboPropKind,
    props: &std::collections::HashMap<&str, &str>,
) -> Option<String> {
    let effective = resolve_effective(content, props)?;
    match kind {
        GazeboPropKind::Float         => expect_float(&effective, tag),
        GazeboPropKind::NonNegFloat   => expect_non_neg_float(&effective, tag),
        GazeboPropKind::PositiveFloat => expect_positive_float(&effective, tag),
        GazeboPropKind::Bool          => expect_bool(&effective, tag),
        GazeboPropKind::Int           => {
            if effective.trim().parse::<i64>().is_err() {
                Some(format!("'{tag}' must be an integer, got '{}'", effective.trim()))
            } else { None }
        }
        GazeboPropKind::AnyString | GazeboPropKind::Lenient => None,
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
    let effective = resolve_effective(value, props)?;
    match (element, attr) {
        (_, "xyz") | (_, "rpy") => expect_n_floats(&effective, 3, attr),
        ("box", "size") => expect_n_floats(&effective, 3, attr)
            .or_else(|| {
                let ok = effective.split_whitespace()
                    .filter_map(|s| s.parse::<f64>().ok())
                    .all(|f| f > 0.0);
                if !ok { Some("'size' values must be positive".to_string()) } else { None }
            }),
        ("color", "rgba") => expect_n_floats(&effective, 4, attr)
            .or_else(|| {
                let vals: Vec<f64> = effective.split_whitespace()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                if vals.len() == 4 && vals.iter().any(|&f| f < 0.0 || f > 1.0) {
                    Some("'rgba' values must be between 0 and 1".into())
                } else { None }
            }),
        (_, "radius") | ("cylinder", "length") => expect_positive_float(&effective, attr),
        (_, "lower") | (_, "upper") | (_, "effort") | (_, "velocity")
        | (_, "damping") | (_, "friction") | (_, "value")
        | (_, "ixx") | (_, "ixy") | (_, "ixz") | (_, "iyy") | (_, "iyz") | (_, "izz")
        | (_, "multiplier") | (_, "offset") => expect_float(&effective, attr),
        _ => None,
    }
}

/// Resolves xacro `${varname}` to the property value for simple single-identifier
/// substitutions. Returns `None` to signal "skip validation" when a `${` pattern
/// is present but unresolvable. Non-xacro values pass through as `Borrowed`.
fn resolve_effective<'a>(
    value: &'a str,
    props: &std::collections::HashMap<&str, &str>,
) -> Option<Cow<'a, str>> {
    if value.contains("${") {
        resolve_simple_xacro(value, props).map(Cow::Owned)
    } else {
        Some(Cow::Borrowed(value))
    }
}

fn resolve_simple_xacro(
    value: &str,
    props: &std::collections::HashMap<&str, &str>,
) -> Option<String> {
    let v = value.trim();
    if !v.starts_with("${") || !v.ends_with('}') {
        return None;
    }
    let inner = &v[2..v.len() - 1];
    if inner.contains(|c: char| matches!(c, '+' | '-' | '*' | '/' | '(' | ')' | ' ' | '\t')) {
        return None;
    }
    let resolved = props.get(inner)?;
    if resolved.contains("${") {
        return None;
    }
    Some(resolved.to_string())
}

fn is_xacro_element(node: &roxmltree::Node) -> bool {
    node.tag_name().namespace().map_or(false, |n| n.contains("xacro"))
        || node.tag_name().name().starts_with("xacro:")
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

fn expect_non_neg_float(value: &str, attr: &str) -> Option<String> {
    let trimmed = value.trim();
    match trimmed.parse::<f64>() {
        Ok(f) if f >= 0.0 => None,
        Ok(_) => Some(format!("'{attr}' must be non-negative")),
        Err(_) => Some(format!("'{attr}' must be a number, got '{trimmed}'")),
    }
}

fn expect_bool(value: &str, attr: &str) -> Option<String> {
    match value.trim() {
        "true" | "false" | "1" | "0" => None,
        v => Some(format!("'{attr}' must be 'true' or 'false', got '{v}'")),
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

fn elem_name_range(text: &str, node: &roxmltree::Node) -> Range {
    let start = node.range().start + 1; // skip '<'
    let name = node.tag_name().name();
    byte_range_to_lsp(text, start..start + name.len())
}

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
    elem_name_range(text, node)
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

fn byte_range_to_lsp(text: &str, range: std::ops::Range<usize>) -> Range {
    crate::document::byte_range_to_lsp(text, range)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document;

    fn diag_messages(text: &str) -> Vec<String> {
        let (doc, mut d) = document::parse(text);
        d.extend(check(&doc, text));
        d.iter().map(|d| d.message.clone()).collect()
    }

    #[test]
    fn tree_isolated_link() {
        // A link with no joints at all should be reported as "no joints"
        let msgs = diag_messages(r#"<?xml version="1.0"?><robot name="r">
          <link name="base_link"/>
          <link name="child"/>
          <link name="orphan"/>
          <joint name="j1" type="fixed">
            <parent link="base_link"/><child link="child"/>
          </joint>
        </robot>"#);
        assert!(msgs.iter().any(|m| m.contains("orphan") && m.contains("no joints")),
            "expected isolated-link diagnostic, got: {:?}", msgs);
    }

    #[test]
    fn tree_multiple_roots() {
        let msgs = diag_messages(r#"<?xml version="1.0"?><robot name="r">
          <link name="root1"/>
          <link name="root2"/>
          <link name="child"/>
          <joint name="j1" type="fixed">
            <parent link="root1"/><child link="child"/>
          </joint>
        </robot>"#);
        assert!(msgs.iter().any(|m| m.contains("root2") && m.contains("root")),
            "expected multiple-roots diagnostic, got: {:?}", msgs);
    }

    #[test]
    fn tree_cycle() {
        let msgs = diag_messages(r#"<?xml version="1.0"?><robot name="r">
          <link name="base_link"/>
          <link name="link_a"/>
          <link name="link_b"/>
          <joint name="j1" type="fixed"><parent link="base_link"/><child link="link_a"/></joint>
          <joint name="j2" type="fixed"><parent link="link_a"/><child link="link_b"/></joint>
          <joint name="j3" type="fixed"><parent link="link_b"/><child link="link_a"/></joint>
        </robot>"#);
        assert!(msgs.iter().any(|m| m.contains("cycle")),
            "expected cycle diagnostic, got: {:?}", msgs);
    }

    #[test]
    fn tag_mismatch_points_to_opening_tag() {
        // The misspelled opening tag is on line 1 (0-indexed), the closing tag matches
        // the original name. The diagnostic should be on the opening tag, not the closing.
        let text = "<robot name=\"r\">\n  <mateaaaaaaa name=\"x\">\n    <color rgba=\"1 1 1 1\"/>\n  </material>\n</robot>\n";
        let (_, diags) = document::parse(text);
        assert_eq!(diags.len(), 1, "expected exactly one diagnostic, got {:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>());
        assert!(diags[0].message.contains("mateaaaaaaa") && diags[0].message.contains("material"),
            "expected mismatch message naming both tags, got: {}", diags[0].message);
        assert_eq!(diags[0].range.start.line, 1, "expected diagnostic on the opening tag line, got: {:?}",
            diags[0].range);
    }

    #[test]
    fn tag_unclosed_points_to_opening() {
        let text = "<robot>\n  <link name=\"foo\">\n</robot>\n";
        let (_, diags) = document::parse(text);
        assert!(diags.iter().any(|d| d.message.contains("never closed") || d.message.contains("Mismatched")),
            "expected unclosed/mismatch diagnostic, got: {:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>());
    }

    #[test]
    fn folding_ranges_cover_multiline_elements() {
        let text = "<robot name=\"r\">\n  <link name=\"a\">\n    <visual>\n      <geometry>\n        <box size=\"1 1 1\"/>\n      </geometry>\n    </visual>\n  </link>\n  <joint name=\"j\" type=\"fixed\"/>\n</robot>\n";
        let ranges = crate::features::folding_ranges(text);
        // Expected foldable elements (multi-line): robot (0..9), link (1..7), visual (2..6), geometry (3..5)
        let starts: Vec<u32> = ranges.iter().map(|r| r.start_line).collect();
        assert!(starts.contains(&0), "robot should fold from line 0, got: {:?}", ranges);
        assert!(starts.contains(&1), "link should fold from line 1, got: {:?}", ranges);
        assert!(starts.contains(&2), "visual should fold from line 2, got: {:?}", ranges);
        assert!(starts.contains(&3), "geometry should fold from line 3, got: {:?}", ranges);
        // The single-line self-closing <joint> at line 8 should NOT produce a fold
        assert!(!starts.contains(&8), "self-closing joint should not fold");
    }

    #[test]
    fn xml_parse_failure_does_not_cascade_undefined_props() {
        // Malformed XML elsewhere in the file must not turn every ${...}
        // reference into a false "Undefined xacro property" — properties
        // are defined at the top but parsing fails on the bad line, so
        // doc.xacro_properties is empty. We rely on parse_ok to skip check().
        let text = r#"<robot xmlns:xacro="http://www.ros.org/wiki/xacro" name="r">
  <xacro:property name="wheel_radius" value="0.033"/>
  <link name="x"><visual><geometry>
    <cylinder radius=aa"ssdada${wheel_radius}s" length="${wheel_thickness}"/>
  </geometry></visual></link>
</robot>"#;
        let (doc, mut diags) = document::parse(text);
        if doc.parse_ok {
            diags.extend(check(&doc, text));
        }
        let undef: Vec<_> = diags.iter().filter(|d| d.message.contains("Undefined xacro property")).collect();
        assert!(undef.is_empty(),
            "no Undefined-xacro-property diagnostics should fire when XML parse fails, got: {:?}",
            undef.iter().map(|d| &d.message).collect::<Vec<_>>());
    }

    #[test]
    fn unclosed_xacro_expression_is_flagged() {
        let text = r#"<robot xmlns:xacro="http://www.ros.org/wiki/xacro" name="r">
  <xacro:property name="wheel_thickness" value="0.026"/>
  <link name="x"><visual><geometry>
    <cylinder radius="${asaso" length="${wheel_thickness}"/>
  </geometry></visual></link>
</robot>"#;
        let (doc, mut d) = document::parse(text);
        d.extend(check(&doc, text));
        assert!(d.iter().any(|m| m.message.contains("Unclosed xacro expression")),
            "expected unclosed-expression diagnostic, got: {:?}",
            d.iter().map(|x| &x.message).collect::<Vec<_>>());
        // wheel_thickness should NOT be flagged as undefined
        assert!(!d.iter().any(|m| m.message.contains("wheel_thickness") && m.message.contains("Undefined")),
            "wheel_thickness should not be flagged");
    }

    #[test]
    fn completion_inside_dollar_brace() {
        use tower_lsp::lsp_types::Position;
        let text = "<robot xmlns:xacro=\"http://www.ros.org/wiki/xacro\" name=\"r\">\n  <xacro:property name=\"wheel_radius\" value=\"0.033\"/>\n  <xacro:property name=\"wheel_thickness\" value=\"0.026\"/>\n  <link name=\"x\"><visual><geometry>\n    <cylinder radius=\"${w}\" length=\"0\"/>\n  </geometry></visual></link>\n</robot>";
        let (doc, _) = document::parse(text);
        // Line 4 (0-indexed): "    <cylinder radius=\"${w}\" length=\"0\"/>"
        // Cursor right after the 'w' (before the closing '}')
        let line = "    <cylinder radius=\"${w}\" length=\"0\"/>";
        let col_after_w = line.find("${w").unwrap() + 3; // position right after 'w'
        let pos = Position::new(4, col_after_w as u32);
        let items = crate::features::completion(&doc, pos, text);
        let labels: Vec<&str> = items.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"wheel_radius"), "expected wheel_radius in completions, got: {:?}", labels);
        assert!(labels.contains(&"wheel_thickness"), "expected wheel_thickness in completions, got: {:?}", labels);
    }

    #[test]
    fn inlay_hints_on_math_expressions() {
        use tower_lsp::lsp_types::{Position, Range};
        let text = r#"<?xml version="1.0"?>
<robot xmlns:xacro="http://www.ros.org/wiki/xacro" name="r">
  <xacro:property name="chassis_length" value="0.335"/>
  <xacro:property name="chassis_width" value="0.265"/>
  <link name="base_link"/>
  <link name="chassis">
    <visual>
      <origin xyz="${chassis_length/2} 0 ${chassis_width/2}"/>
    </visual>
  </link>
</robot>"#;
        let (doc, _) = document::parse(text);
        let full = Range::new(Position::new(0, 0), Position::new(100, 0));
        let hints = crate::features::inlay_hints(&doc, text, full);
        // Expect two hints: 0.335/2 = 0.1675 and 0.265/2 = 0.1325
        assert_eq!(hints.len(), 2, "expected 2 hints, got {:?}", hints.iter().map(|h| &h.label).collect::<Vec<_>>());
        let labels: Vec<String> = hints.iter().map(|h| match &h.label {
            tower_lsp::lsp_types::InlayHintLabel::String(s) => s.clone(),
            _ => String::new(),
        }).collect();
        assert!(labels.iter().any(|l| l.contains("0.1675")), "expected 0.1675, got: {:?}", labels);
        assert!(labels.iter().any(|l| l.contains("0.1325")), "expected 0.1325, got: {:?}", labels);
    }

    #[test]
    fn tree_valid_chain() {
        let msgs = diag_messages(r#"<?xml version="1.0"?><robot name="r">
          <link name="base_link"/>
          <link name="link_a"/>
          <link name="link_b"/>
          <joint name="j1" type="fixed"><parent link="base_link"/><child link="link_a"/></joint>
          <joint name="j2" type="fixed"><parent link="link_a"/><child link="link_b"/></joint>
        </robot>"#);
        let tree_diags: Vec<_> = msgs.iter()
            .filter(|m| m.contains("root") || m.contains("connected") || m.contains("cycle"))
            .collect();
        assert!(tree_diags.is_empty(), "expected no tree diagnostics, got: {:?}", tree_diags);
    }
}
