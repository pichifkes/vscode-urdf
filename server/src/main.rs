use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

mod document;
mod diagnostics;
mod features;

#[derive(Debug)]
struct Backend {
    client: Client,
    docs: Arc<Mutex<HashMap<Url, (document::Document, String)>>>,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                completion_provider: Some(CompletionOptions::default()),
                document_symbol_provider: Some(OneOf::Left(true)),
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
        let map = self.docs.lock().await;
        Ok(map.get(&uri).and_then(|(doc, _)| features::hover(doc, pos)))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let pos = params.text_document_position_params.position;
        let uri = params.text_document_position_params.text_document.uri.clone();
        let map = self.docs.lock().await;
        Ok(map.get(&uri).and_then(|(doc, _)| {
            features::goto_definition(doc, pos).map(|range| {
                GotoDefinitionResponse::Scalar(Location { uri: uri.clone(), range })
            })
        }))
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

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

impl Backend {
    async fn validate(&self, uri: Url, text: String) {
        let (doc, mut diags) = document::parse(&text);
        diags.extend(diagnostics::check(&doc, &text));
        diags.extend(diagnostics::check_schema(&text));
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
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}
