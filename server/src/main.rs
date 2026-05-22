use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

mod document;
mod diagnostics;
mod features;
mod xacro_eval;

// ── Workspace index ──────────────────────────────────────────────────────────

/// Entity names and definition locations known across the entire workspace.
/// Populated from all open and scanned URDF/xacro files.
#[derive(Debug, Default)]
struct WorkspaceIndex {
    // Fast membership sets (for diagnostic suppression)
    link_names:  std::collections::HashSet<String>,
    joint_names: std::collections::HashSet<String>,
    prop_names:  std::collections::HashSet<String>,
    // Definition locations (for cross-file goto-def and hover)
    link_locations:  HashMap<String, (Url, Range)>,
    joint_locations: HashMap<String, (Url, Range)>,
    /// Per-file summary used to clean up stale entries on update.
    per_file: HashMap<Url, FileSummary>,
}

/// Per-file entity list carrying both the name and its definition range.
#[derive(Debug, Default)]
struct FileSummary {
    links:  Vec<(String, Range)>,
    joints: Vec<(String, Range)>,
    props:  Vec<(String, Range)>,
}

impl WorkspaceIndex {
    fn upsert(&mut self, uri: Url, summary: FileSummary) {
        if let Some(old) = self.per_file.remove(&uri) {
            for (n, _) in old.links  { self.link_names.remove(&n); self.link_locations.remove(&n); }
            for (n, _) in old.joints { self.joint_names.remove(&n); self.joint_locations.remove(&n); }
            for (n, _) in old.props  { self.prop_names.remove(&n); }
        }
        for (n, r) in &summary.links {
            self.link_names.insert(n.clone());
            self.link_locations.insert(n.clone(), (uri.clone(), *r));
        }
        for (n, r) in &summary.joints {
            self.joint_names.insert(n.clone());
            self.joint_locations.insert(n.clone(), (uri.clone(), *r));
        }
        for (n, _) in &summary.props {
            self.prop_names.insert(n.clone());
        }
        self.per_file.insert(uri, summary);
    }
}

// ── Workspace file scanner ───────────────────────────────────────────────────

fn collect_urdf_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return; };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_urdf_files(&p, out);
        } else if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
            if name.ends_with(".urdf") || name.ends_with(".xacro") {
                out.push(p);
            }
        }
    }
}

/// Synchronously scan workspace roots, parse every URDF/xacro file found,
/// and return their entity summaries. Intended to run inside `spawn_blocking`.
fn scan_workspace_sync(roots: Vec<std::path::PathBuf>) -> Vec<(Url, FileSummary)> {
    let mut files = Vec::new();
    for root in &roots {
        collect_urdf_files(root, &mut files);
    }
    files
        .into_iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            let (doc, _) = document::parse(&text);
            let uri = Url::from_file_path(&path).ok()?;
            Some((uri, summary_from_doc(&doc)))
        })
        .collect()
}

fn summary_from_doc(doc: &document::Document) -> FileSummary {
    FileSummary {
        links:  doc.links.iter().map(|l| (l.name.clone(), l.range)).collect(),
        joints: doc.joints.iter().map(|j| (j.name.clone(), j.range)).collect(),
        props:  doc.xacro_properties.iter().map(|p| (p.name.clone(), p.range)).collect(),
    }
}

// ── LSP backend ──────────────────────────────────────────────────────────────

#[derive(Debug)]
struct Backend {
    client: Client,
    docs: Arc<Mutex<HashMap<Url, (document::Document, String)>>>,
    workspace_roots: Arc<Mutex<Vec<std::path::PathBuf>>>,
    workspace_index: Arc<tokio::sync::RwLock<WorkspaceIndex>>,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Prefer UTF-8 position encoding so `Position.character` is a byte offset
        // matching our internal UTF-8 text storage. Fall back to UTF-16 (the LSP
        // default) if the client doesn't advertise UTF-8 support.
        let position_encoding = params
            .capabilities
            .general
            .as_ref()
            .and_then(|g| g.position_encodings.as_ref())
            .and_then(|encs| encs.iter().find(|e| **e == PositionEncodingKind::UTF8).cloned());

        // Save workspace roots for the initial file scan in `initialized()`.
        {
            let mut roots = self.workspace_roots.lock().await;
            if let Some(folders) = params.workspace_folders.as_deref() {
                for f in folders {
                    if let Ok(p) = f.uri.to_file_path() {
                        roots.push(p);
                    }
                }
            } else if let Some(uri) = params.root_uri.as_ref() {
                if let Ok(p) = uri.to_file_path() {
                    roots.push(p);
                }
            }
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                position_encoding,
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![
                        "$".to_string(),
                        "{".to_string(),
                        "\"".to_string(),
                        "<".to_string(),
                    ]),
                    ..CompletionOptions::default()
                }),
                document_symbol_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Left(true)),
                inlay_hint_provider: Some(OneOf::Left(true)),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
                color_provider: Some(ColorProviderCapability::Simple(true)),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo {
                name: "urdf-lsp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "urdf-lsp initialized")
            .await;

        let roots = self.workspace_roots.lock().await.clone();
        if roots.is_empty() {
            return;
        }

        // Spawn a background task: scan workspace files, populate the index,
        // then re-publish diagnostics for any already-open files so cross-file
        // refs resolve immediately on startup.
        let ws_idx   = self.workspace_index.clone();
        let docs_map = self.docs.clone();
        let client   = self.client.clone();

        tokio::spawn(async move {
            let Ok(entries) = tokio::task::spawn_blocking(move || {
                scan_workspace_sync(roots)
            }).await else { return; };

            {
                let mut idx = ws_idx.write().await;
                for (uri, summary) in entries {
                    idx.upsert(uri, summary);
                }
            }

            // Re-run diagnostics for every file that was opened before the scan
            // finished, so cross-file false positives are cleared immediately.
            let open: Vec<(Url, String)> = {
                let map = docs_map.lock().await;
                map.iter().map(|(u, (_, t))| (u.clone(), t.clone())).collect()
            };

            for (uri, text) in open {
                let (doc, mut diags) = document::parse(&text);
                if doc.parse_ok {
                    let ws = {
                        let idx = ws_idx.read().await;
                        diagnostics::WorkspaceNames {
                            links:  idx.link_names.clone(),
                            joints: idx.joint_names.clone(),
                            props:  idx.prop_names.clone(),
                        }
                    };
                    diags.extend(diagnostics::check(&doc, &text, Some(&ws)));
                    if let Ok(ref xml) = roxmltree::Document::parse(&text) {
                        diags.extend(diagnostics::check_schema(xml, &text));
                    }
                }
                client.publish_diagnostics(uri, diags, None).await;
            }
        });
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.validate(params.text_document.uri, params.text_document.text).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        self.validate(
            params.text_document.uri,
            params.content_changes.into_iter().last().map(|c| c.text).unwrap_or_default(),
        )
        .await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let pos = params.text_document_position_params.position;
        let uri = params.text_document_position_params.text_document.uri;

        // Phase 1: same-file hover; also capture the ref name for phase 2.
        let (result, ref_info) = {
            let map = self.docs.lock().await;
            match map.get(&uri) {
                None => {
                    self.client.log_message(MessageType::WARNING, format!("hover: doc not found for {uri}")).await;
                    (None, None)
                }
                Some((doc, _)) => {
                    let h = features::hover(doc, pos);
                    let ri = if h.is_none() { features::entity_at(doc, pos) } else { None };
                    self.client.log_message(MessageType::INFO, format!(
                        "hover: pos={:?} same_file={} entity_at={:?}",
                        pos, h.is_some(), ri.as_ref().map(|(n, _)| n.as_str())
                    )).await;
                    (h, ri)
                }
            }
        };
        if result.is_some() { return Ok(result); }

        // Phase 2: cross-file — look up the referenced entity in the workspace index.
        if let Some((name, kind)) = ref_info {
            let idx = self.workspace_index.read().await;
            let loc = match kind {
                features::ItemKind::Link  => idx.link_locations.get(&name),
                features::ItemKind::Joint => idx.joint_locations.get(&name),
            };
            self.client.log_message(MessageType::INFO, format!(
                "hover cross-file: name={name} found_in_index={}", loc.is_some()
            )).await;
            if let Some((def_uri, _)) = loc {
                return Ok(Some(Self::cross_file_hover(&name, &kind, def_uri)));
            }
        }
        Ok(None)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let pos = params.text_document_position_params.position;
        let uri = params.text_document_position_params.text_document.uri.clone();

        // Phase 1: same-file goto-def; also capture ref name for phase 2.
        let (result, ref_info) = {
            let map = self.docs.lock().await;
            match map.get(&uri) {
                None => (None, None),
                Some((doc, _)) => {
                    let r = features::goto_definition(doc, pos).map(|range| {
                        GotoDefinitionResponse::Scalar(Location { uri: uri.clone(), range })
                    });
                    let ri = if r.is_none() { features::entity_at(doc, pos) } else { None };
                    (r, ri)
                }
            }
        };
        if result.is_some() { return Ok(result); }

        // Phase 2: cross-file — navigate to the definition in another workspace file.
        if let Some((name, kind)) = ref_info {
            let idx = self.workspace_index.read().await;
            let loc = match kind {
                features::ItemKind::Link  => idx.link_locations.get(&name),
                features::ItemKind::Joint => idx.joint_locations.get(&name),
            };
            if let Some((def_uri, def_range)) = loc {
                return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                    uri: def_uri.clone(),
                    range: *def_range,
                })));
            }
        }
        Ok(None)
    }

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> Result<Option<CompletionResponse>> {
        let pos = params.text_document_position.position;
        let uri = params.text_document_position.text_document.uri;
        let map = self.docs.lock().await;
        Ok(map.get(&uri).and_then(|(doc, text)| {
            let items = features::completion(doc, pos, text);
            if items.is_empty() { None } else { Some(CompletionResponse::Array(items)) }
        }))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri;
        let map = self.docs.lock().await;
        Ok(map.get(&uri).and_then(|(doc, _)| {
            let symbols = features::document_symbols(doc);
            if symbols.is_empty() { None } else { Some(DocumentSymbolResponse::Nested(symbols)) }
        }))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let pos = params.text_document_position.position;
        let uri = params.text_document_position.text_document.uri;
        let map = self.docs.lock().await;
        Ok(map.get(&uri).map(|(doc, _)| {
            features::references(doc, pos)
                .into_iter()
                .map(|range| Location { uri: uri.clone(), range })
                .collect()
        }))
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let pos = params.text_document_position.position;
        let new_name = params.new_name.clone();
        let uri = params.text_document_position.text_document.uri.clone();
        let map = self.docs.lock().await;
        Ok(map.get(&uri).and_then(|(doc, _)| {
            let changes = features::rename(doc, pos, &new_name);
            if changes.is_empty() { return None; }
            let edits = changes.into_iter()
                .map(|(range, new_text)| TextEdit { range, new_text })
                .collect::<Vec<_>>();
            let mut map = std::collections::HashMap::new();
            map.insert(uri.clone(), edits);
            Some(WorkspaceEdit { changes: Some(map), ..WorkspaceEdit::default() })
        }))
    }

    async fn prepare_rename(&self, params: TextDocumentPositionParams) -> Result<Option<PrepareRenameResponse>> {
        let pos = params.position;
        let uri = params.text_document.uri;
        let map = self.docs.lock().await;
        Ok(map.get(&uri).and_then(|(doc, _)| {
            features::prepare_rename(doc, pos).map(|(range, placeholder)| {
                PrepareRenameResponse::RangeWithPlaceholder { range, placeholder }
            })
        }))
    }

    async fn document_color(&self, params: DocumentColorParams) -> Result<Vec<ColorInformation>> {
        let uri = params.text_document.uri;
        let map = self.docs.lock().await;
        Ok(map.get(&uri).map(|(_, text)| features::document_colors(text)).unwrap_or_default())
    }

    async fn color_presentation(&self, params: ColorPresentationParams) -> Result<Vec<ColorPresentation>> {
        Ok(features::color_presentations(params.color))
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        let uri = params.text_document.uri;
        let map = self.docs.lock().await;
        Ok(map.get(&uri).map(|(_, text)| features::folding_ranges(text)))
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;
        let diags = params.context.diagnostics;
        let map = self.docs.lock().await;
        Ok(map.get(&uri).map(|(doc, text)| {
            features::code_actions(doc, text, &diags, &uri)
        }))
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = params.text_document.uri;
        let range = params.range;
        let map = self.docs.lock().await;
        Ok(map.get(&uri).map(|(doc, text)| features::inlay_hints(doc, text, range)))
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

impl Backend {
    fn cross_file_hover(name: &str, kind: &features::ItemKind, def_uri: &Url) -> Hover {
        let file = def_uri.path_segments()
            .and_then(|mut s| s.next_back())
            .unwrap_or("another file");
        let label = match kind {
            features::ItemKind::Link  => format!("**Link** `{name}` — defined in `{file}`"),
            features::ItemKind::Joint => format!("**Joint** `{name}` — defined in `{file}`"),
        };
        Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: label,
            }),
            range: None,
        }
    }

    async fn validate(&self, uri: Url, text: String) {
        let (doc, mut diags) = document::parse(&text);

        // Step 1: update workspace index with this file's entities.
        {
            let summary = summary_from_doc(&doc);
            let mut idx = self.workspace_index.write().await;
            idx.upsert(uri.clone(), summary);
        }

        // Step 2: snapshot workspace names (drop lock before running diagnostics).
        let ws = {
            let idx = self.workspace_index.read().await;
            diagnostics::WorkspaceNames {
                links:  idx.link_names.clone(),
                joints: idx.joint_names.clone(),
                props:  idx.prop_names.clone(),
            }
        };

        // Step 3: run diagnostics with workspace context.
        if doc.parse_ok {
            diags.extend(diagnostics::check(&doc, &text, Some(&ws)));
            if let Ok(ref xml) = roxmltree::Document::parse(&text) {
                diags.extend(diagnostics::check_schema(xml, &text));
            }
        }

        {
            let mut map = self.docs.lock().await;
            map.insert(uri.clone(), (doc, text));
        }
        self.client.publish_diagnostics(uri, diags, None).await;
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(|client| Backend {
        client,
        docs: Arc::new(Mutex::new(HashMap::new())),
        workspace_roots: Arc::new(Mutex::new(Vec::new())),
        workspace_index: Arc::new(tokio::sync::RwLock::new(WorkspaceIndex::default())),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}
