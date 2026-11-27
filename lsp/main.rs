mod extra;
mod reminders;

use lazy_static::lazy_static;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

lazy_static! {
    static ref PENDING_PATTERN: Regex = Regex::new(r"^(\s*)☐\s+(.*)$").unwrap();
    static ref DONE_PATTERN: Regex = Regex::new(r"^(\s*)✔\s+(.*)$").unwrap();
    static ref CANCELLED_PATTERN: Regex = Regex::new(r"^(\s*)✘\s+(.*)$").unwrap();
    static ref DONE_TAG: Regex = Regex::new(r"\s*@done\([^)]+\)").unwrap();
    static ref CANCELLED_TAG: Regex = Regex::new(r"\s*@cancelled\([^)]+\)").unwrap();
    static ref TAG_PATTERN: Regex = Regex::new(r"@(\w+)").unwrap();
}

#[derive(Debug)]
struct DocumentState {
    content: String,
}

struct Backend {
    client: Client,
    documents: Arc<tokio::sync::RwLock<HashMap<String, DocumentState>>>,
    workspace_folders: tokio::sync::RwLock<Vec<String>>,
    notified: Arc<tokio::sync::Mutex<HashSet<String>>>,
    reminder_notify: Arc<tokio::sync::Notify>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            documents: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            workspace_folders: tokio::sync::RwLock::new(Vec::new()),
            notified: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
            reminder_notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    fn get_current_timestamp() -> String {
        chrono::Local::now().format("%y-%m-%d %H:%M").to_string()
    }

    fn get_line_at_position(&self, content: &str, position: Position) -> Option<(String, usize, usize)> {
        let lines: Vec<&str> = content.lines().collect();
        if position.line as usize >= lines.len() {
            return None;
        }
        let line = lines[position.line as usize].to_string();
        let line_start = lines[..position.line as usize]
            .iter()
            .map(|l| l.len() + 1)
            .sum();
        let line_end = line_start + line.len();
        Some((line, line_start, line_end))
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        if let Some(folders) = &params.workspace_folders {
            let paths = folders
                .iter()
                .filter_map(|folder| folder.uri.to_file_path().ok())
                .map(|path| path.to_string_lossy().to_string())
                .collect::<Vec<_>>();
            *self.workspace_folders.write().await = paths;
        } else if let Some(root) = &params.root_uri {
            if let Ok(path) = root.to_file_path() {
                *self.workspace_folders.write().await = vec![path.to_string_lossy().to_string()];
            }
        }
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(true),
                        })),
                        ..Default::default()
                    },
                )),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec!["@".to_string()]),
                    ..Default::default()
                }),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "PlainTasks LSP initialized")
            .await;

        let client = self.client.clone();
        let folders = self.workspace_folders.read().await.clone();
        let documents = self.documents.clone();
        let notified = self.notified.clone();
        let notify = self.reminder_notify.clone();
        tokio::spawn(async move {
            loop {
                let open = documents
                    .read()
                    .await
                    .iter()
                    .map(|(uri, doc)| (uri.clone(), doc.content.clone()))
                    .collect::<Vec<_>>();
                let mut due = reminders::collect(&folders, &open);
                {
                    let seen = notified.lock().await;
                    due.retain(|task| !seen.contains(&task.key));
                }
                let Some(next_alarm) = due.iter().map(|task| task.alarm_at).min() else {
                    notify.notified().await;
                    continue;
                };
                let delay = next_alarm
                    .signed_duration_since(chrono::Local::now())
                    .to_std()
                    .unwrap_or_default();
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {
                        let open = documents
                            .read()
                            .await
                            .iter()
                            .map(|(uri, doc)| (uri.clone(), doc.content.clone()))
                            .collect::<Vec<_>>();
                        let mut ready = reminders::collect(&folders, &open);
                        let now = chrono::Local::now();
                        let mut seen = notified.lock().await;
                        ready.retain(|task| {
                            task.alarm_at <= now && !seen.contains(&task.key)
                        });
                        for task in &ready {
                            seen.insert(task.key.clone());
                        }
                        drop(seen);
                        if ready.is_empty() {
                            continue;
                        }
                        reminders::send_notification(&ready);
                        client
                            .show_message(
                                MessageType::INFO,
                                reminders::title(&ready),
                            )
                            .await;
                    }
                    _ = notify.notified() => {}
                }
            }
        });
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        let content = params.text_document.text;

        self.documents.write().await.insert(
            uri,
            DocumentState { content },
        );
        self.reminder_notify.notify_one();
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let uri_key = uri.to_string();
        let Some(change) = params.content_changes.first() else {
            return;
        };
        let content = change.text.clone();
        if let Some(doc) = self.documents.write().await.get_mut(&uri_key) {
            doc.content = content.clone();
        }

        let mut edits = Vec::new();
        for (line_number, line) in content.lines().enumerate() {
            if let Some(resolved) = extra::resolve_date_tags(line) {
                edits.push(TextEdit {
                    range: Range {
                        start: Position {
                            line: line_number as u32,
                            character: 0,
                        },
                        end: Position {
                            line: line_number as u32,
                            character: u32::MAX,
                        },
                    },
                    new_text: resolved,
                });
            }
        }
        if !edits.is_empty() {
            let edit = WorkspaceEdit {
                changes: Some(HashMap::from([(uri, edits)])),
                ..Default::default()
            };
            if let Err(error) = self.client.apply_edit(edit).await {
                self.client
                    .log_message(MessageType::WARNING, format!("auto-resolve failed: {error}"))
                    .await;
            }
        }
        self.reminder_notify.notify_one();
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        self.documents.write().await.remove(&uri);
        self.reminder_notify.notify_one();
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        if let Some(text) = params.text {
            if let Some(doc) = self.documents.write().await.get_mut(&uri) {
                doc.content = text;
            }
        }
        self.reminder_notify.notify_one();
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri.to_string();
        let documents = self.documents.read().await;

        if let Some(doc) = documents.get(&uri) {
            // Extract all existing tags from the document
            let mut tags = std::collections::HashSet::new();
            for cap in TAG_PATTERN.captures_iter(&doc.content) {
                if let Some(tag) = cap.get(1) {
                    tags.insert(tag.as_str().to_string());
                }
            }

            // Common tags to suggest
            let common_tags = vec![
                "today", "tomorrow", "tonight", "yesterday",
                "next_week", "next_month", "next_year",
                "next_monday", "next_tuesday", "next_wednesday",
                "next_thursday", "next_friday", "next_saturday", "next_sunday",
                "end_of_week", "end_of_month",
                "in_3_days", "in_2_weeks",
                "due", "section", "project",
                "high", "medium", "low", "critical",
                "done", "cancelled", "started", "est", "lasted",
                "blocked", "waiting", "legacy_upstream_tag",
                "specialized_agent_test_tag",
            ];

            for tag in common_tags {
                tags.insert(tag.to_string());
            }

            let items: Vec<CompletionItem> = tags
                .into_iter()
                .map(|tag| CompletionItem {
                    label: format!("@{}", tag),
                    kind: Some(CompletionItemKind::KEYWORD),
                    insert_text: Some(tag),
                    ..Default::default()
                })
                .collect();

            return Ok(Some(CompletionResponse::Array(items)));
        }

        Ok(None)
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri.to_string();
        let position = params.range.start;
        let documents = self.documents.read().await;

        if let Some(doc) = documents.get(&uri) {
            if let Some((line, _line_start, _line_end)) = self.get_line_at_position(&doc.content, position) {
                let mut actions = Vec::new();

                // Archive completed/cancelled tasks
                if DONE_PATTERN.is_match(&line)
                    || CANCELLED_PATTERN.is_match(&line)
                    || line.trim_end().eq_ignore_ascii_case("Archive:")
                {
                    if let Some(updated) = extra::archive_completed(&doc.content) {
                        let line_count = doc.content.lines().count() as u32;
                        actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                            title: "Archive completed tasks".to_string(),
                            kind: Some(CodeActionKind::REFACTOR),
                            edit: Some(WorkspaceEdit {
                                changes: Some(HashMap::from([(
                                    params.text_document.uri.clone(),
                                    vec![TextEdit {
                                        range: Range {
                                            start: Position { line: 0, character: 0 },
                                            end: Position { line: line_count, character: 0 },
                                        },
                                        new_text: updated,
                                    }],
                                )])),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }));
                    }
                }

                // Check for pending task
                if PENDING_PATTERN.is_match(&line) {
                    // Mark as Done
                    if let Some(caps) = PENDING_PATTERN.captures(&line) {
                        let indent = caps.get(1).map_or("", |m| m.as_str());
                        let content = caps.get(2).map_or("", |m| m.as_str());
                        let new_line = format!("{}✔ {} @done({})", indent, content, Self::get_current_timestamp());

                        actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                            title: "Mark as Done".to_string(),
                            kind: Some(CodeActionKind::QUICKFIX),
                            is_preferred: Some(true),
                            edit: Some(WorkspaceEdit {
                                changes: Some(HashMap::from([(
                                    params.text_document.uri.clone(),
                                    vec![TextEdit {
                                        range: Range {
                                            start: Position { line: position.line, character: 0 },
                                            end: Position { line: position.line, character: u32::MAX },
                                        },
                                        new_text: new_line,
                                    }],
                                )])),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }));
                    }

                    // Mark as Cancelled
                    if let Some(caps) = PENDING_PATTERN.captures(&line) {
                        let indent = caps.get(1).map_or("", |m| m.as_str());
                        let content = caps.get(2).map_or("", |m| m.as_str());
                        let new_line = format!("{}✘ {} @cancelled({})", indent, content, Self::get_current_timestamp());

                        actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                            title: "Mark as Cancelled".to_string(),
                            kind: Some(CodeActionKind::QUICKFIX),
                            edit: Some(WorkspaceEdit {
                                changes: Some(HashMap::from([(
                                    params.text_document.uri.clone(),
                                    vec![TextEdit {
                                        range: Range {
                                            start: Position { line: position.line, character: 0 },
                                            end: Position { line: position.line, character: u32::MAX },
                                        },
                                        new_text: new_line,
                                    }],
                                )])),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }));
                    }
                }

                // Check for done task
                if DONE_PATTERN.is_match(&line) {
                    if let Some(caps) = DONE_PATTERN.captures(&line) {
                        let indent = caps.get(1).map_or("", |m| m.as_str());
                        let content = caps.get(2).map_or("", |m| m.as_str());
                        let content_clean = DONE_TAG.replace_all(content, "").trim().to_string();
                        let new_line = format!("{}☐ {}", indent, content_clean);

                        actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                            title: "Revert to Pending".to_string(),
                            kind: Some(CodeActionKind::QUICKFIX),
                            is_preferred: Some(true),
                            edit: Some(WorkspaceEdit {
                                changes: Some(HashMap::from([(
                                    params.text_document.uri.clone(),
                                    vec![TextEdit {
                                        range: Range {
                                            start: Position { line: position.line, character: 0 },
                                            end: Position { line: position.line, character: u32::MAX },
                                        },
                                        new_text: new_line,
                                    }],
                                )])),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }));
                    }
                }

                // Check for cancelled task
                if CANCELLED_PATTERN.is_match(&line) {
                    if let Some(caps) = CANCELLED_PATTERN.captures(&line) {
                        let indent = caps.get(1).map_or("", |m| m.as_str());
                        let content = caps.get(2).map_or("", |m| m.as_str());
                        let content_clean = CANCELLED_TAG.replace_all(content, "").trim().to_string();
                        let new_line = format!("{}☐ {}", indent, content_clean);

                        actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                            title: "Revert to Pending".to_string(),
                            kind: Some(CodeActionKind::QUICKFIX),
                            is_preferred: Some(true),
                            edit: Some(WorkspaceEdit {
                                changes: Some(HashMap::from([(
                                    params.text_document.uri.clone(),
                                    vec![TextEdit {
                                        range: Range {
                                            start: Position { line: position.line, character: 0 },
                                            end: Position { line: position.line, character: u32::MAX },
                                        },
                                        new_text: new_line,
                                    }],
                                )])),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }));
                    }
                }

                // Check if line is NOT a todo and NOT a project - offer to convert
                let is_todo = PENDING_PATTERN.is_match(&line) ||
                              DONE_PATTERN.is_match(&line) ||
                              CANCELLED_PATTERN.is_match(&line);
                let is_project = line.trim_end().ends_with(':');

                if !is_todo && !is_project {
                    // Extract current indentation
                    let indent = line.chars()
                        .take_while(|c| c.is_whitespace())
                        .collect::<String>();

                    // Get content after indentation
                    let content = line.trim_start();

                    // Create new line with todo symbol
                    let new_line = if content.is_empty() {
                        format!("{}☐ ", indent)
                    } else {
                        format!("{}☐ {}", indent, content)
                    };

                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: "Convert to Todo item".to_string(),
                        kind: Some(CodeActionKind::REFACTOR),
                        is_preferred: Some(true),
                        edit: Some(WorkspaceEdit {
                            changes: Some(HashMap::from([(
                                params.text_document.uri.clone(),
                                vec![TextEdit {
                                    range: Range {
                                        start: Position { line: position.line, character: 0 },
                                        end: Position { line: position.line, character: u32::MAX },
                                    },
                                    new_text: new_line,
                                }],
                            )])),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }));
                }

                // Resolve date keywords in the whole document
                if let Some(updated) = extra::resolve_file_dates(&doc.content) {
                    let line_count = doc.content.lines().count() as u32;
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: "Resolve date keywords in file".to_string(),
                        kind: Some(CodeActionKind::REFACTOR),
                        edit: Some(WorkspaceEdit {
                            changes: Some(HashMap::from([(
                                params.text_document.uri.clone(),
                                vec![TextEdit {
                                    range: Range {
                                        start: Position { line: 0, character: 0 },
                                        end: Position { line: line_count, character: 0 },
                                    },
                                    new_text: updated,
                                }],
                            )])),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }));
                }

                // Always offer "Insert New Todo Below" action
                // Extract indentation from current line to maintain nesting
                let indent = line.chars()
                    .take_while(|c| c.is_whitespace())
                    .collect::<String>();

                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Insert new Todo item below".to_string(),
                    kind: Some(CodeActionKind::REFACTOR),
                    edit: Some(WorkspaceEdit {
                        changes: Some(HashMap::from([(
                            params.text_document.uri.clone(),
                            vec![TextEdit {
                                range: Range {
                                    start: Position { line: position.line + 1, character: 0 },
                                    end: Position { line: position.line + 1, character: 0 },
                                },
                                new_text: format!("{}☐ \n", indent),
                            }],
                        )])),
                        ..Default::default()
                    }),
                    ..Default::default()
                }));

                if !actions.is_empty() {
                    return Ok(Some(actions));
                }
            }
        }

        Ok(None)
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| Backend::new(client));
    Server::new(stdin, stdout, socket).serve(service).await;
}
