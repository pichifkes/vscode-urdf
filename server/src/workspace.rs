use std::collections::HashMap;
use tower_lsp::lsp_types::{Range, Url};

use crate::document::Document;

/// Per-file summary of named entities — what `WorkspaceIndex` stores keyed by URI
/// so it can prune the right entries when a file is re-upserted.
#[derive(Debug, Default)]
pub struct FileSummary {
    pub links:  Vec<(String, Range)>,
    pub joints: Vec<(String, Range)>,
    pub props:  Vec<(String, Range)>,
}

impl FileSummary {
    pub fn from_doc(doc: &Document) -> Self {
        Self {
            links:  doc.links.iter().map(|l| (l.name.clone(), l.range)).collect(),
            joints: doc.joints.iter().map(|j| (j.name.clone(), j.range)).collect(),
            props:  doc.xacro_properties.iter().map(|p| (p.name.clone(), p.range)).collect(),
        }
    }
}

/// Workspace-wide index of named entities, populated from all open and scanned
/// URDF/xacro files. Both diagnostic suppression (membership) and cross-file
/// hover/goto-def (location lookup) read from this single source of truth.
///
/// Each name maps to **all** of its definitions across the workspace — same-name
/// definitions in different files are preserved rather than silently overwritten.
#[derive(Debug, Default)]
pub struct WorkspaceIndex {
    links:    HashMap<String, Vec<(Url, Range)>>,
    joints:   HashMap<String, Vec<(Url, Range)>>,
    props:    HashMap<String, Vec<(Url, Range)>>,
    per_file: HashMap<Url, FileSummary>,
}

impl WorkspaceIndex {
    pub fn has_link(&self, name: &str)  -> bool { self.links.contains_key(name) }
    pub fn has_joint(&self, name: &str) -> bool { self.joints.contains_key(name) }
    pub fn has_prop(&self, name: &str)  -> bool { self.props.contains_key(name) }

    /// All definitions of a link by this name across the workspace.
    /// Empty slice if no file defines it.
    pub fn link_defs(&self, name: &str)  -> &[(Url, Range)] { slice(&self.links,  name) }
    pub fn joint_defs(&self, name: &str) -> &[(Url, Range)] { slice(&self.joints, name) }
    /// Locations are recorded but not yet consumed by hover/goto-def — kept for
    /// API symmetry with [`Self::link_defs`] / [`Self::joint_defs`] and as the
    /// foundation for cross-file xacro property navigation.
    #[allow(dead_code)]
    pub fn prop_defs(&self, name: &str)  -> &[(Url, Range)] { slice(&self.props,  name) }

    /// Replace this file's entry in the index. Old entries from `uri` (if any)
    /// are removed first, so a rename/delete inside the file does not leak
    /// stale entries.
    pub fn upsert(&mut self, uri: Url, summary: FileSummary) {
        if let Some(old) = self.per_file.remove(&uri) {
            prune(&mut self.links,  &uri, &old.links);
            prune(&mut self.joints, &uri, &old.joints);
            prune(&mut self.props,  &uri, &old.props);
        }
        add(&mut self.links,  &uri, &summary.links);
        add(&mut self.joints, &uri, &summary.joints);
        add(&mut self.props,  &uri, &summary.props);
        self.per_file.insert(uri, summary);
    }
}

fn slice<'a>(map: &'a HashMap<String, Vec<(Url, Range)>>, name: &str) -> &'a [(Url, Range)] {
    map.get(name).map(|v| v.as_slice()).unwrap_or(&[])
}

fn add(
    map: &mut HashMap<String, Vec<(Url, Range)>>,
    uri: &Url,
    entries: &[(String, Range)],
) {
    for (n, r) in entries {
        map.entry(n.clone()).or_default().push((uri.clone(), *r));
    }
}

fn prune(
    map: &mut HashMap<String, Vec<(Url, Range)>>,
    uri: &Url,
    entries: &[(String, Range)],
) {
    for (n, _) in entries {
        if let Some(v) = map.get_mut(n) {
            v.retain(|(u, _)| u != uri);
            if v.is_empty() {
                map.remove(n);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::Position;

    fn r(line: u32) -> Range {
        Range::new(Position::new(line, 0), Position::new(line, 1))
    }

    fn uri(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn upsert_populates_lookup() {
        let mut idx = WorkspaceIndex::default();
        let u = uri("file:///a.urdf");
        idx.upsert(u.clone(), FileSummary {
            links:  vec![("base_link".into(), r(1))],
            joints: vec![("j1".into(), r(2))],
            props:  vec![("mass".into(), r(3))],
        });

        assert!(idx.has_link("base_link"));
        assert!(idx.has_joint("j1"));
        assert!(idx.has_prop("mass"));
        assert_eq!(idx.link_defs("base_link"), &[(u.clone(), r(1))]);
        assert_eq!(idx.joint_defs("j1"),       &[(u,         r(2))]);
        assert!(idx.link_defs("missing").is_empty());
    }

    #[test]
    fn upsert_replaces_previous_summary_for_same_uri() {
        let mut idx = WorkspaceIndex::default();
        let u = uri("file:///a.urdf");
        idx.upsert(u.clone(), FileSummary {
            links: vec![("old".into(), r(1))],
            ..Default::default()
        });
        idx.upsert(u.clone(), FileSummary {
            links: vec![("new".into(), r(2))],
            ..Default::default()
        });
        assert!(!idx.has_link("old"), "old entry should be pruned on re-upsert");
        assert!(idx.has_link("new"));
        assert_eq!(idx.link_defs("new"), &[(u, r(2))]);
    }

    #[test]
    fn same_name_in_two_files_keeps_both_definitions() {
        // Two files both defining "base_link" — index must keep both locations,
        // not silently overwrite. goto-def will return both.
        let mut idx = WorkspaceIndex::default();
        let a = uri("file:///a.urdf");
        let b = uri("file:///b.urdf");
        idx.upsert(a.clone(), FileSummary { links: vec![("base_link".into(), r(1))], ..Default::default() });
        idx.upsert(b.clone(), FileSummary { links: vec![("base_link".into(), r(5))], ..Default::default() });

        let defs = idx.link_defs("base_link");
        assert_eq!(defs.len(), 2, "expected both definitions, got {defs:?}");
        assert!(defs.contains(&(a, r(1))));
        assert!(defs.contains(&(b, r(5))));
    }

    #[test]
    fn removing_one_file_leaves_other_definition() {
        let mut idx = WorkspaceIndex::default();
        let a = uri("file:///a.urdf");
        let b = uri("file:///b.urdf");
        idx.upsert(a.clone(), FileSummary { links: vec![("base_link".into(), r(1))], ..Default::default() });
        idx.upsert(b.clone(), FileSummary { links: vec![("base_link".into(), r(5))], ..Default::default() });

        // Re-upsert `a` with an empty summary (simulates the link being removed from file a).
        idx.upsert(a, FileSummary::default());

        assert!(idx.has_link("base_link"));
        assert_eq!(idx.link_defs("base_link"), &[(b, r(5))]);
    }
}
