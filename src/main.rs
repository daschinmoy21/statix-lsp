use dashmap::DashMap;
use rnix::{Root, WalkEvent};
use statix::{LINTS, Lint};
use std::collections::HashMap;
use std::sync::OnceLock;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct NativeSuggestion {
    range: Range,
    fix: String,
}

// Core server state shared across all LSP handlers.
struct Backend {
    // Client handle lets us send logs/diagnostics back to the editor.
    client: Client,
    // Stored document text + version so we can publish versioned diagnostics.
    documents: DashMap<Url, DocumentState>,
    lint_map: OnceLock<HashMap<rnix::SyntaxKind, Vec<&'static Box<dyn Lint>>>>,
}

#[derive(Debug, Clone)]
struct DocumentState {
    text: String,
    version: i32,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(
        &self,
        _: InitializeParams,
    ) -> tower_lsp::jsonrpc::Result<InitializeResult> {
        // Startup contract: this tells the editor exactly what we support.
        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "Native Statix LSP Solution".to_string(),
                version: Some("0.1.0".to_string()),
            }),
            capabilities: ServerCapabilities {
                // FULL sync is simpler for prototype; INCREMENTAL is a later optimization.
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "Native Statix LSP Solution Initialized!")
            .await;
    }

    async fn shutdown(&self) -> tower_lsp::jsonrpc::Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        // On open we cache the text snapshot and immediately lint.
        let uri = params.text_document.uri;
        let state = DocumentState {
            text: params.text_document.text,
            version: params.text_document.version,
        };
        self.documents.insert(uri.clone(), state.clone());
        self.lint(uri, state).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        // For FULL sync, first content change carries the full latest document.
        if let Some(change) = params.content_changes.into_iter().next() {
            let uri = params.text_document.uri;
            let state = DocumentState {
                text: change.text,
                version: params.text_document.version,
            };
            self.documents.insert(uri.clone(), state.clone());
            self.lint(uri, state).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        //removes documents from the Dashmap on close 
        self.documents.remove(&params.text_document.uri);
    }

    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> tower_lsp::jsonrpc::Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;
        let mut actions = Vec::new();

        for diag in params.context.diagnostics {
            if let Some(data) = diag.data {
                if let Ok(suggestion) = serde_json::from_value::<NativeSuggestion>(data) {
                    let action = CodeAction {
                        title: format!("Apply Statix fix: {}", suggestion.fix),
                        kind: Some(CodeActionKind::QUICKFIX),
                        edit: Some(WorkspaceEdit {
                            changes: Some(
                                [(
                                    uri.clone(),
                                    vec![TextEdit {
                                        range: suggestion.range,
                                        new_text: suggestion.fix,
                                    }],
                                )]
                                .into_iter()
                                .collect(),
                            ),
                            ..Default::default()
                        }),
                        is_preferred: Some(true),
                        ..Default::default()
                    };
                    actions.push(CodeActionOrCommand::CodeAction(action));
                }
            }
        }

        Ok(Some(actions))
    }
}

impl Backend {
    // Build the SyntaxKind -> rules lookup once.
    fn lint_map(&self) -> &HashMap<rnix::SyntaxKind, Vec<&'static Box<dyn Lint>>> {
        //  This avoids scanning all  rules for every node in every lint run.
        self.lint_map.get_or_init(|| {
            let mut map: HashMap<rnix::SyntaxKind, Vec<&'static Box<dyn Lint>>> = HashMap::new();
            for rule in LINTS.iter() {
                for kind in rule.match_kind() {
                    map.entry(kind).or_default().push(rule);
                }
            }
            map
        })
    }

    async fn lint(&self, uri: Url, doc: DocumentState) {
        // Keep lint path observable for debugging latency in editor output.
        self.client
            .log_message(MessageType::LOG, &format!("Linting natively: {}", uri))
            .await;

        let mut diagnostics = Vec::new();
        let text = doc.text;

        //  Parse the text directly into an AST using the rnix library
        let parsed = Root::parse(&text);

        //  Add parser syntax errors first.
        for err in parsed.errors() {
            let statix_report = statix::Report::from_parse_err(err);
            for diag in statix_report.diagnostics {
                diagnostics.push(Diagnostic {
                    range: self.byte_range_to_lsp_range(
                        &text,
                        usize::from(diag.at.start()),
                        usize::from(diag.at.end()),
                    ),
                    severity: Some(DiagnosticSeverity::ERROR),
                    source: Some("statix-parser".to_string()),
                    message: self.beautify_message(&diag.message),
                    ..Default::default()
                });
            }
        }

        // Walk the AST and apply statix lints even if there are parse errors,
        // since partial ASTs can still contain valid, lintable subtrees.
        for event in parsed.syntax().preorder_with_tokens() {
            if let WalkEvent::Enter(child) = event {
                if let Some(rules) = self.lint_map().get(&child.kind()) {
                    for rule in rules {
                        if let Some(report) = rule.validate(&child) {
                            for diag in report.diagnostics {
                                let suggestion_data = diag.suggestion.map(|s| {
                                    serde_json::to_value(NativeSuggestion {
                                        range: self.byte_range_to_lsp_range(
                                            &text,
                                            usize::from(s.at.start()),
                                            usize::from(s.at.end()),
                                        ),
                                        fix: s.fix.to_string(), // SyntaxElement implements Display natively
                                    })
                                    .unwrap()
                                });

                                diagnostics.push(Diagnostic {
                                    range: self.byte_range_to_lsp_range(
                                        &text,
                                        usize::from(diag.at.start()),
                                        usize::from(diag.at.end()),
                                    ),
                                    severity: Some(DiagnosticSeverity::WARNING),
                                    code: i32::try_from(report.code)
                                        .ok()
                                        .map(NumberOrString::Number),
                                    source: Some("statix-lint".to_string()),
                                    message: self.beautify_message(&format!(
                                        "{}: {}",
                                        report.note, diag.message
                                    )),
                                    data: suggestion_data,
                                    ..Default::default()
                                });
                            }
                        }
                    }
                }
            }
        }

        // Sending a document version prevents stale diagnostics racing newer ones.
        self.client
            .publish_diagnostics(uri, diagnostics, Some(doc.version))
            .await;
    }

    fn beautify_message(&self, msg: &str) -> String {
        msg.replace("TOKEN_ASSIGN", "`=`")
            .replace("TOKEN_IDENT", "identifier")
            .replace("TOKEN_IN", "`in`")
            .replace("TOKEN_LET", "`let`")
            .replace("TOKEN_SEMICOLON", "`;`")
            .replace("TOKEN_DOT", "`.`")
            .replace("TOKEN_CUR_B_OPEN", "`{`")
            .replace("TOKEN_CUR_B_CLOSE", "`}`")
            .replace("TOKEN_CURLY_B_OPEN", "`{`")
            .replace("TOKEN_CURLY_B_CLOSE", "`}`")
            .replace("TOKEN_PAREN_OPEN", "`(`")
            .replace("TOKEN_PAREN_CLOSE", "`)`")
            .replace("TOKEN_BRACK_OPEN", "`[`")
            .replace("TOKEN_BRACK_CLOSE", "`]`")
            .replace("TOKEN_WHITESPACE", "whitespace")
            .replace("TOKEN_COMMENT", "comment")
            .replace("TOKEN_QUESTION", "`?`")
            .replace("TOKEN_COMMA", "`,`")
            .replace("TOKEN_COLON", "`:`")
            .replace("TOKEN_ELLIPSIS", "`...`")
            .replace("TOKEN_AT", "`@`")
    }

    // Helper function because Native ASTs often return byte offsets (e.g., 0..15),
    // but LSP needs Line/Column (e.g., Line 0, Col 15).
    fn byte_range_to_lsp_range(&self, text: &str, start_byte: usize, end_byte: usize) -> Range {
        Range {
            start: self.byte_to_position(text, start_byte),
            end: self.byte_to_position(text, end_byte),
        }
    }

    // Converts byte offsets from rnix/statix into LSP UTF-16 line/character positions.
    // This stays panic-safe by avoiding string slicing at arbitrary byte indexes.
    fn byte_to_position(&self, text: &str, byte_offset: usize) -> Position {
        let clamped = byte_offset.min(text.len());
        let mut line = 0;
        let mut character = 0u32;

        for (i, c) in text.char_indices() {
            if i >= clamped {
                break;
            }
            if c == '\r' {
                continue;
            }
            if c == '\n' {
                line += 1;
                character = 0;
            } else {
                // LSP uses UTF-16 code units for `character`.
                character += c.len_utf16() as u32;
            }
        }
        Position { line, character }
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::build(|client| Backend {
        client,
        documents: DashMap::new(),
        // Lazy init avoids paying rule indexing cost at startup.
        lint_map: OnceLock::new(),
    })
    .finish();

    Server::new(stdin, stdout, socket).serve(service).await;
}
