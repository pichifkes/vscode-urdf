use tower_lsp::lsp_types::{Position, Range};

#[derive(Debug, Clone)]
pub struct Document {
    pub links: Vec<NamedItem>,
    pub joints: Vec<Joint>,
    pub materials: Vec<NamedItem>,
    pub xacro_properties: Vec<NamedItem>,
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

pub fn parse(text: &str) -> Document {
    let mut doc = Document {
        links: Vec::new(),
        joints: Vec::new(),
        materials: Vec::new(),
        xacro_properties: Vec::new(),
    };

    let xml = match roxmltree::Document::parse(text) {
        Ok(d) => d,
        Err(_) => return doc,
    };

    let root = xml.root_element();

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

    doc
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

fn byte_offset_to_position(text: &str, offset: usize) -> Position {
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

fn byte_range_to_lsp(text: &str, range: std::ops::Range<usize>) -> Range {
    Range {
        start: byte_offset_to_position(text, range.start),
        end: byte_offset_to_position(text, range.end),
    }
}
