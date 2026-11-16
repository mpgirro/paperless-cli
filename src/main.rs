use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use paperless_cli::api::{ApiClient, HttpMethod, ReqwestTransport, Transport};
use paperless_cli::config::{load_config, save_config, AppConfig, AppPaths, OutputMode};
use paperless_cli::demo::{demo_config, DemoTransport};
use paperless_cli::error::AppError;
use paperless_cli::pdf::{read_local_pdf_info, read_local_pdf_text};
use paperless_cli::render::render_output;
use paperless_cli::security::{SecurityAgentProfile, SecurityAuditor};
use paperless_cli::services::{
    api_passthrough, audit_state, autocomplete_search, bulk_download, bulk_download_zip, bulk_edit,
    bulk_operations, connection_info, create_correspondent, create_document_type, create_tag,
    dashboard, delete_correspondent, delete_document, delete_document_type, delete_tag,
    download_document, download_preview, download_thumbnail, edit_document, get_correspondent,
    get_document, get_document_content, get_document_type, get_tag, get_task, init_connection,
    list_correspondents, list_document_types, list_documents, list_tags, list_tasks, list_trash,
    operational_overview, persist_session, ping, query_search, sanitize_filename, search_documents,
    status, trash_action, unique, update_document, update_tag, upload_document, BulkEditRequest,
    BulkOperation, BulkPermissions, DocumentQuery, OutputEnvelope, TagUpdateRequest, UpdateRequest,
    UploadRequest,
};
use paperless_cli::tui::run_tui;
use serde_json::{json, Value};

#[derive(Parser, Debug)]
#[command(
    name = "paperless",
    version,
    about = "Rust TUI and LLM-friendly client for Paperless-ngx"
)]
struct Cli {
    #[arg(long, global = true, value_enum, default_value_t = OutputMode::Markdown)]
    output: OutputMode,
    #[arg(long, global = true)]
    json: bool,
    #[arg(short = 'q', long, global = true, default_value_t = false)]
    quiet: bool,
    #[arg(long = "no-color", global = true, default_value_t = false)]
    no_color: bool,
    #[arg(short = 'u', long, global = true)]
    url: Option<String>,
    /// HTTP timeout in seconds (default 30, 0 disables it). Uploads use it only for connecting.
    #[arg(long, global = true, value_name = "SECS")]
    timeout: Option<u64>,
    #[arg(long, global = true, default_value_t = false)]
    demo: bool,
    #[command(subcommand)]
    command: Option<RootCommand>,
}

#[derive(Subcommand, Debug)]
enum RootCommand {
    Login(LoginArgs),
    #[command(name = "project", subcommand)]
    Project(ProjectCommand),
    #[command(name = "document", alias = "documents", subcommand)]
    Documents(DocumentsCommand),
    #[command(name = "search", subcommand)]
    Search(SearchCommand),
    #[command(name = "config", subcommand)]
    Config(ConfigCommand),
    #[command(name = "pdf", subcommand)]
    Pdf(PdfCommand),
    #[command(name = "task", alias = "tasks", subcommand)]
    Tasks(TasksCommand),
    #[command(name = "tag", alias = "tags", subcommand)]
    Tags(TagsCommand),
    #[command(name = "correspondent", alias = "correspondents", subcommand)]
    Correspondents(CorrespondentsCommand),
    #[command(name = "doctype", alias = "document-types", subcommand)]
    DocumentTypes(DocumentTypesCommand),
    #[command(name = "export", subcommand)]
    Export(ExportCommand),
    /// List, restore or permanently delete documents in the trash.
    #[command(name = "trash", subcommand)]
    Trash(TrashCommand),
    /// Call any API endpoint with the stored URL and token and print the response JSON.
    Api(ApiArgs),
    /// Fetch status, statistics, and tasks for a full operational overview.
    Dashboard,
    /// Check server health and authentication using only GET /api/status/.
    Status,
}

#[derive(Subcommand, Debug)]
enum ProjectCommand {
    #[command(name = "login", alias = "init")]
    Login(LoginArgs),
    Info,
    Ping,
}

#[derive(Subcommand, Debug)]
enum ConfigCommand {
    #[command(name = "set-url")]
    SetUrl(ConfigValueArgs),
    #[command(name = "set-token")]
    SetToken(ConfigValueArgs),
}

#[derive(Subcommand, Debug)]
enum PdfCommand {
    Read(PdfPathArgs),
    Info(PdfPathArgs),
}

#[derive(Args, Debug)]
struct LoginArgs {
    #[arg(long)]
    url: Option<String>,
    #[arg(long)]
    token: Option<String>,
    #[arg(long)]
    username: Option<String>,
    #[arg(long)]
    password: Option<String>,
}

#[derive(Subcommand, Debug)]
enum DocumentsCommand {
    List(DocumentListArgs),
    Get(IdArgs),
    Content(IdArgs),
    Upload(DocumentUploadArgs),
    Download(DocumentDownloadArgs),
    Preview(DocumentAssetArgs),
    Thumb(DocumentAssetArgs),
    Edit(DocumentEditArgs),
    Update(DocumentUpdateArgs),
    Delete(IdArgs),
    Search(DocumentSearchArgs),
    /// Change many documents at once.
    ///
    /// Sends one request per operation, in this order: tags, correspondent, document type,
    /// permissions (all through documents/bulk_edit/), then reprocess (documents/reprocess/).
    /// Stops at the first failed request.
    Bulk(DocumentBulkArgs),
}

#[derive(Args, Debug)]
struct IdArgs {
    id: u64,
}

#[derive(Args, Debug)]
struct DocumentListArgs {
    #[arg(long)]
    query: Option<String>,
    #[command(flatten)]
    filters: DocumentFilterArgs,
}

#[derive(Args, Debug)]
struct DocumentFilterArgs {
    #[arg(long)]
    tag: Option<String>,
    #[arg(long)]
    tag_id: Option<u64>,
    #[arg(long)]
    correspondent: Option<String>,
    #[arg(long)]
    correspondent_id: Option<u64>,
    #[arg(long, alias = "type")]
    document_type: Option<String>,
    #[arg(long)]
    type_id: Option<u64>,
    #[arg(long)]
    created_after: Option<String>,
    #[arg(long)]
    created_before: Option<String>,
    #[arg(long, default_value = "-created")]
    order_by: String,
    #[arg(long, default_value_t = 25)]
    page_size: u64,
    #[arg(long, default_value_t = 1)]
    page: u64,
}

#[derive(Args, Debug)]
struct DocumentUploadArgs {
    path: PathBuf,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    correspondent_id: Option<u64>,
    #[arg(long)]
    type_id: Option<u64>,
    #[arg(long = "tag-id")]
    tag_ids: Vec<u64>,
}

#[derive(Args, Debug)]
struct DocumentDownloadArgs {
    id: u64,
    #[arg(long, default_value = ".")]
    output_dir: PathBuf,
    #[arg(long, default_value_t = false)]
    original: bool,
}

#[derive(Args, Debug)]
struct DocumentAssetArgs {
    id: u64,
    #[arg(long, default_value = ".")]
    output_dir: PathBuf,
}

#[derive(Args, Debug)]
struct DocumentUpdateArgs {
    id: u64,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    correspondent_id: Option<u64>,
    #[arg(long, conflicts_with = "correspondent_id")]
    no_correspondent: bool,
    #[arg(long)]
    type_id: Option<u64>,
    #[arg(long, conflicts_with = "type_id")]
    no_type: bool,
    #[arg(long)]
    storage_path_id: Option<u64>,
    #[arg(long, conflicts_with = "storage_path_id")]
    no_storage_path: bool,
    #[arg(long = "tag-id")]
    tag_ids: Vec<u64>,
    #[arg(long, conflicts_with = "tag_ids")]
    no_tags: bool,
    #[arg(long)]
    created: Option<String>,
}

impl DocumentUpdateArgs {
    fn to_request(&self) -> UpdateRequest {
        UpdateRequest {
            title: self.title.clone(),
            correspondent_id: clearable(self.correspondent_id, self.no_correspondent),
            document_type_id: clearable(self.type_id, self.no_type),
            storage_path_id: clearable(self.storage_path_id, self.no_storage_path),
            tag_ids: if self.no_tags {
                Some(Vec::new())
            } else if self.tag_ids.is_empty() {
                None
            } else {
                Some(self.tag_ids.clone())
            },
            created: self.created.clone(),
            custom_fields: None,
        }
    }
}

#[derive(Args, Debug)]
struct DocumentEditArgs {
    id: u64,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    correspondent_id: Option<u64>,
    #[arg(long, conflicts_with = "correspondent_id")]
    no_correspondent: bool,
    #[arg(long)]
    type_id: Option<u64>,
    #[arg(long, conflicts_with = "type_id")]
    no_type: bool,
    #[arg(long)]
    storage_path_id: Option<u64>,
    #[arg(long, conflicts_with = "storage_path_id")]
    no_storage_path: bool,
    #[arg(long)]
    created: Option<String>,
    #[arg(long = "add-tag")]
    add_tags: Vec<String>,
    #[arg(long = "remove-tag")]
    remove_tags: Vec<String>,
    #[arg(long, conflicts_with_all = ["add_tags", "remove_tags"])]
    no_tags: bool,
}

impl DocumentEditArgs {
    fn to_request(&self) -> UpdateRequest {
        UpdateRequest {
            title: self.title.clone(),
            correspondent_id: clearable(self.correspondent_id, self.no_correspondent),
            document_type_id: clearable(self.type_id, self.no_type),
            storage_path_id: clearable(self.storage_path_id, self.no_storage_path),
            tag_ids: self.no_tags.then(Vec::new),
            created: self.created.clone(),
            custom_fields: None,
        }
    }
}

#[derive(Args, Debug)]
struct DocumentBulkArgs {
    /// Document IDs; `-` reads whitespace-separated IDs from stdin.
    #[arg(required = true)]
    ids: Vec<String>,
    /// Tag ID to add, repeatable.
    #[arg(long = "add-tag", value_name = "ID")]
    add_tags: Vec<u64>,
    /// Tag ID to remove, repeatable.
    #[arg(long = "remove-tag", value_name = "ID")]
    remove_tags: Vec<u64>,
    /// Correspondent ID, or `none` to clear it.
    #[arg(long, value_name = "ID|none", value_parser = parse_id_or_none)]
    correspondent: Option<IdOrNone>,
    /// Document type ID, or `none` to clear it.
    #[arg(long = "type", value_name = "ID|none", value_parser = parse_id_or_none)]
    document_type: Option<IdOrNone>,
    /// Re-run parsing and OCR through documents/reprocess/.
    #[arg(long)]
    reprocess: bool,
    /// Set the owner and replace the given permission lists.
    #[arg(long)]
    set_permissions: bool,
    /// Owner user ID, or `none` for no owner, which makes the documents visible to all users.
    #[arg(
        long,
        value_name = "ID|none",
        value_parser = parse_id_or_none,
        requires = "set_permissions",
        required_if_eq("set_permissions", "true")
    )]
    owner: Option<IdOrNone>,
    /// User IDs that may view, comma-separated, or `none` to clear them. Omitted: left unchanged.
    #[arg(long, value_name = "IDS|none", value_parser = parse_id_list, requires = "set_permissions")]
    view_users: Option<IdList>,
    /// Group IDs that may view, comma-separated, or `none` to clear them. Omitted: left unchanged.
    #[arg(long, value_name = "IDS|none", value_parser = parse_id_list, requires = "set_permissions")]
    view_groups: Option<IdList>,
    /// User IDs that may change, comma-separated, or `none` to clear them. Omitted: left unchanged.
    #[arg(long, value_name = "IDS|none", value_parser = parse_id_list, requires = "set_permissions")]
    change_users: Option<IdList>,
    /// Group IDs that may change, comma-separated, or `none` to clear them. Omitted: left unchanged.
    #[arg(long, value_name = "IDS|none", value_parser = parse_id_list, requires = "set_permissions")]
    change_groups: Option<IdList>,
    /// Print the requests (method, path, body) as JSON and send nothing.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Clone, Copy, Debug)]
struct IdOrNone(Option<u64>);

#[derive(Clone, Debug)]
struct IdList(Vec<u64>);

fn parse_id_list(value: &str) -> Result<IdList, String> {
    if value.eq_ignore_ascii_case("none") {
        return Ok(IdList(Vec::new()));
    }
    value
        .split(',')
        .map(|id| id.trim().parse())
        .collect::<Result<_, _>>()
        .map(IdList)
        .map_err(|_| "expected comma-separated IDs or `none`".to_string())
}

fn parse_id_or_none(value: &str) -> Result<IdOrNone, String> {
    if value.eq_ignore_ascii_case("none") {
        return Ok(IdOrNone(None));
    }
    value
        .parse()
        .map(|id| IdOrNone(Some(id)))
        .map_err(|_| "expected an ID or `none`".to_string())
}

impl DocumentBulkArgs {
    fn to_request(&self) -> BulkEditRequest {
        BulkEditRequest {
            add_tags: self.add_tags.clone(),
            remove_tags: self.remove_tags.clone(),
            correspondent: self.correspondent.map(|id| id.0),
            document_type: self.document_type.map(|id| id.0),
            permissions: self.set_permissions.then(|| BulkPermissions {
                owner: self.owner.and_then(|id| id.0),
                view_users: self.view_users.clone().map(|ids| ids.0),
                view_groups: self.view_groups.clone().map(|ids| ids.0),
                change_users: self.change_users.clone().map(|ids| ids.0),
                change_groups: self.change_groups.clone().map(|ids| ids.0),
            }),
            reprocess: self.reprocess,
        }
    }

    fn operations(&self) -> Result<Vec<BulkOperation>, AppError> {
        bulk_operations(&read_document_ids(&self.ids)?, &self.to_request())
    }
}

fn read_document_ids(args: &[String]) -> Result<Vec<u64>, AppError> {
    let mut ids = Vec::new();
    for arg in args {
        if arg == "-" {
            let input = io::read_to_string(io::stdin())?;
            for token in input.split_whitespace() {
                ids.push(parse_document_id(token)?);
            }
        } else {
            ids.push(parse_document_id(arg)?);
        }
    }
    Ok(ids)
}

fn run_trash<T: Transport>(
    client: &ApiClient<T>,
    command: TrashCommand,
) -> Result<(&'static str, Value), AppError> {
    match command {
        TrashCommand::List => Ok(("trash list", list_trash(client)?)),
        TrashCommand::Restore(args) => Ok((
            "trash restore",
            trash_action(client, "restore", &read_document_ids(&args.ids)?)?,
        )),
        TrashCommand::Empty(args) => {
            let ids = unique(&read_document_ids(&args.documents.ids)?);
            if !ids.is_empty() && !args.yes {
                confirm_empty_trash(&ids)?;
            }
            Ok(("trash empty", trash_action(client, "empty", &ids)?))
        }
    }
}

fn confirm_empty_trash(ids: &[u64]) -> Result<(), AppError> {
    if !io::stdin().is_terminal() {
        return Err(AppError::Message(
            "Refusing to empty the trash without --yes: stdin is not a terminal.".to_string(),
        ));
    }
    let list = ids
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    eprint!("Permanently delete documents {list}? This cannot be undone. [y/N] ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(AppError::Message(
            "Aborted. Nothing was deleted.".to_string(),
        ))
    }
}

fn parse_document_id(value: &str) -> Result<u64, AppError> {
    value
        .parse()
        .map_err(|_| AppError::Message(format!("Invalid document ID: {value}")))
}

fn clearable(value: Option<u64>, clear: bool) -> Option<Option<u64>> {
    if clear {
        Some(None)
    } else {
        value.map(Some)
    }
}

#[derive(Args, Debug)]
struct DocumentSearchArgs {
    query: String,
    #[command(flatten)]
    filters: DocumentFilterArgs,
}

#[derive(Subcommand, Debug)]
enum SearchCommand {
    Query(SearchQueryArgs),
    Autocomplete(AutocompleteArgs),
}

#[derive(Args, Debug)]
struct SearchQueryArgs {
    query: String,
    #[arg(long, default_value_t = 25)]
    page_size: u64,
    #[arg(long, default_value_t = 1)]
    page: u64,
}

#[derive(Args, Debug)]
struct AutocompleteArgs {
    term: String,
    #[arg(long, default_value_t = 10)]
    limit: u64,
}

#[derive(Subcommand, Debug)]
enum TrashCommand {
    /// List trashed documents with id, title and deletion time.
    List,
    /// Restore documents from the trash.
    Restore(TrashIdsArgs),
    /// Permanently delete documents from the trash. Asks first unless --yes is given.
    Empty(TrashEmptyArgs),
}

#[derive(Args, Debug)]
struct TrashIdsArgs {
    /// Document IDs; `-` reads whitespace-separated IDs from stdin.
    #[arg(required = true)]
    ids: Vec<String>,
}

#[derive(Args, Debug)]
struct TrashEmptyArgs {
    #[command(flatten)]
    documents: TrashIdsArgs,
    /// Skip the confirmation prompt. Required when stdin is not a terminal.
    #[arg(long)]
    yes: bool,
}

#[derive(Subcommand, Debug)]
enum TasksCommand {
    List,
    Get(IdArgs),
}

#[derive(Subcommand, Debug)]
enum TagsCommand {
    List,
    Get(IdArgs),
    Create(TagCreateArgs),
    Edit(TagEditArgs),
    Delete(IdArgs),
}

#[derive(Args, Debug)]
struct TagCreateArgs {
    name: String,
    #[arg(long, default_value = "#a6cee3")]
    color: String,
    #[arg(long, default_value_t = false)]
    inbox: bool,
}

#[derive(Args, Debug)]
struct TagEditArgs {
    id: u64,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    color: Option<String>,
    #[arg(long)]
    inbox: Option<bool>,
}

#[derive(Subcommand, Debug)]
enum CorrespondentsCommand {
    List,
    Get(IdArgs),
    Create(MatcherCreateArgs),
    Delete(IdArgs),
}

#[derive(Subcommand, Debug)]
enum DocumentTypesCommand {
    List,
    Get(IdArgs),
    Create(MatcherCreateArgs),
    Delete(IdArgs),
}

#[derive(Args, Debug)]
struct MatcherCreateArgs {
    name: String,
    #[arg(long, default_value = "")]
    matcher: String,
}

#[derive(Subcommand, Debug)]
enum ExportCommand {
    Bulk(BulkArgs),
}

#[derive(Args, Debug)]
struct BulkArgs {
    ids: Vec<u64>,
    #[arg(long, default_value = ".")]
    output_dir: PathBuf,
    #[arg(long, default_value_t = false)]
    original: bool,
    #[arg(long, default_value_t = false)]
    zip: bool,
}

#[derive(Args, Debug)]
struct ApiArgs {
    /// GET, POST, PATCH, PUT or DELETE (case-insensitive).
    #[arg(value_parser = parse_http_method)]
    method: HttpMethod,
    /// Path relative to /api/, e.g. trash/ or documents/123/history/.
    path: String,
    /// JSON request body, or @file.json to read it from a file.
    #[arg(long)]
    data: Option<String>,
    /// Query parameter, repeatable.
    #[arg(long, value_name = "KEY=VALUE", value_parser = parse_query_pair)]
    query: Vec<(String, String)>,
    /// Follow `next` links of a paginated GET and print the merged `results`.
    #[arg(long, default_value_t = false)]
    all: bool,
}

#[derive(Args, Debug)]
struct ConfigValueArgs {
    value: String,
}

#[derive(Args, Debug)]
struct PdfPathArgs {
    path: PathBuf,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), AppError> {
    let cli = Cli::parse();
    let paths = AppPaths::default();
    let output = if cli.json {
        OutputMode::Json
    } else {
        cli.output
    };

    if let Some(RootCommand::Documents(DocumentsCommand::Bulk(args))) = &cli.command {
        if args.dry_run {
            let mut bytes = serde_json::to_vec_pretty(&args.operations()?)?;
            bytes.push(b'\n');
            io::stdout().write_all(&bytes)?;
            return Ok(());
        }
    }

    if cli.demo {
        return match cli.command {
            None => run_demo_interactive(output, cli.no_color),
            Some(command) => run_demo_command(command, output, &paths, cli.quiet),
        };
    }

    let overrides = Overrides {
        url: cli.url.as_deref(),
        timeout: cli.timeout,
    };
    match cli.command {
        None => run_interactive(&paths, overrides, cli.no_color),
        Some(command) => run_command(command, output, &paths, overrides, cli.quiet),
    }
}

fn run_interactive(
    paths: &AppPaths,
    overrides: Overrides,
    _no_color: bool,
) -> Result<(), AppError> {
    let config = load_config_with_override(paths, overrides)?;
    let transport = ReqwestTransport::new(config.clone())?;
    let client = ApiClient::new(transport);
    let auditor = SecurityAuditor::new(
        SecurityAgentProfile::security_reviewer(),
        Duration::from_secs(5),
    );
    let shared_state = Arc::new(Mutex::new(audit_state(paths, Some(&config), None)));
    let findings = auditor.review_once(&shared_state.lock().unwrap().clone());
    let receiver = auditor.spawn(shared_state);
    let snapshot = dashboard(&client, &config, findings)?;
    run_tui(client, snapshot, receiver)
}

fn run_demo_interactive(output: OutputMode, _no_color: bool) -> Result<(), AppError> {
    let config = demo_config(output);
    let client = ApiClient::new(DemoTransport::new());
    let snapshot = dashboard(&client, &config, Vec::new())?;
    let (_sender, receiver) = std::sync::mpsc::channel();
    run_tui(client, snapshot, receiver)
}

fn run_command(
    command: RootCommand,
    output: OutputMode,
    paths: &AppPaths,
    overrides: Overrides,
    quiet: bool,
) -> Result<(), AppError> {
    let security_profile = SecurityAgentProfile::security_reviewer();
    let auditor = SecurityAuditor::new(security_profile, Duration::from_secs(60));

    let envelope = match command {
        RootCommand::Login(args) => {
            login_envelope(args, output, paths, overrides.timeout, &auditor)?
        }
        RootCommand::Project(project_command) => match project_command {
            ProjectCommand::Login(args) => {
                login_envelope(args, output, paths, overrides.timeout, &auditor)?
            }
            ProjectCommand::Info => {
                let config = load_config_with_override(paths, overrides)?;
                let client = client_for_config(&config)?;
                let findings = auditor.review_once(&audit_state(paths, Some(&config), None));
                OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "project info".to_string(),
                    data: connection_info(&client, &config, paths)?,
                    security: findings,
                }
            }
            ProjectCommand::Ping => {
                let config = load_config_with_override(paths, overrides)?;
                let client = client_for_config(&config)?;
                let findings = auditor.review_once(&audit_state(paths, Some(&config), None));
                OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "project ping".to_string(),
                    data: ping(&client, &config)?,
                    security: findings,
                }
            }
        },
        RootCommand::Config(config_command) => match config_command {
            ConfigCommand::SetUrl(args) => {
                let config = set_config_url(paths, args.value)?;
                OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "config set-url".to_string(),
                    data: serde_json::json!({
                        "status": "ok",
                        "url": config.base_url,
                        "token": config.masked_token(),
                    }),
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                }
            }
            ConfigCommand::SetToken(args) => {
                let config = set_config_token(paths, args.value)?;
                OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "config set-token".to_string(),
                    data: serde_json::json!({
                        "status": "ok",
                        "url": config.base_url,
                        "token": config.masked_token(),
                    }),
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                }
            }
        },
        RootCommand::Pdf(pdf_command) => match pdf_command {
            PdfCommand::Read(args) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "pdf read".to_string(),
                data: serde_json::json!({
                    "path": args.path,
                    "text": read_local_pdf_text(&args.path).map_err(|error| AppError::Message(error.to_string()))?,
                }),
                security: auditor.review_once(&audit_state(paths, None, None)),
            },
            PdfCommand::Info(args) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "pdf info".to_string(),
                data: serde_json::to_value(
                    read_local_pdf_info(&args.path)
                        .map_err(|error| AppError::Message(error.to_string()))?,
                )?,
                security: auditor.review_once(&audit_state(paths, None, None)),
            },
        },
        RootCommand::Documents(documents_command) => {
            let config = load_config_with_override(paths, overrides)?;
            let client = client_for_config(&config)?;
            let mut session = paperless_cli::config::load_session(paths);
            let envelope = match documents_command {
                DocumentsCommand::List(args) => {
                    let query = document_query_from_list_args(&args);
                    if let Some(search) = &query.query {
                        session.last_query = search.clone();
                    }
                    OutputEnvelope {
                        mode: output_name(output).to_string(),
                        command: "documents list".to_string(),
                        data: list_documents(&client, &query)?,
                        security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                    }
                }
                DocumentsCommand::Get(args) => OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "documents get".to_string(),
                    data: get_document(&client, args.id)?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                },
                DocumentsCommand::Content(args) => OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "documents content".to_string(),
                    data: get_document_content(&client, args.id)?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                },
                DocumentsCommand::Upload(args) => OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "documents upload".to_string(),
                    data: upload_document(
                        &client,
                        &UploadRequest {
                            path: args.path,
                            title: args.title,
                            correspondent_id: args.correspondent_id,
                            document_type_id: args.type_id,
                            tag_ids: args.tag_ids,
                        },
                    )?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                },
                DocumentsCommand::Download(args) => {
                    let path =
                        download_document(&client, args.id, &args.output_dir, args.original)?;
                    OutputEnvelope {
                        mode: output_name(output).to_string(),
                        command: "documents download".to_string(),
                        data: serde_json::json!({
                            "doc_id": args.id,
                            "path": path,
                            "status": "ok",
                        }),
                        security: auditor.review_once(&audit_state(
                            paths,
                            Some(&config),
                            Some(path.to_string_lossy().to_string()),
                        )),
                    }
                }
                DocumentsCommand::Preview(args) => {
                    let path = download_preview(&client, args.id, &args.output_dir)?;
                    OutputEnvelope {
                        mode: output_name(output).to_string(),
                        command: "documents preview".to_string(),
                        data: serde_json::json!({
                            "doc_id": args.id,
                            "path": path,
                            "status": "ok",
                            "asset": "preview",
                        }),
                        security: auditor.review_once(&audit_state(
                            paths,
                            Some(&config),
                            Some(path.to_string_lossy().to_string()),
                        )),
                    }
                }
                DocumentsCommand::Thumb(args) => {
                    let path = download_thumbnail(&client, args.id, &args.output_dir)?;
                    OutputEnvelope {
                        mode: output_name(output).to_string(),
                        command: "documents thumb".to_string(),
                        data: serde_json::json!({
                            "doc_id": args.id,
                            "path": path,
                            "status": "ok",
                            "asset": "thumb",
                        }),
                        security: auditor.review_once(&audit_state(
                            paths,
                            Some(&config),
                            Some(path.to_string_lossy().to_string()),
                        )),
                    }
                }
                DocumentsCommand::Edit(args) => OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "documents edit".to_string(),
                    data: edit_document(
                        &client,
                        args.id,
                        &args.to_request(),
                        &args.add_tags,
                        &args.remove_tags,
                    )?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                },
                DocumentsCommand::Update(args) => OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "documents update".to_string(),
                    data: update_document(&client, args.id, &args.to_request())?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                },
                DocumentsCommand::Bulk(args) => OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "documents bulk".to_string(),
                    data: bulk_edit(&client, &args.operations()?)?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                },
                DocumentsCommand::Delete(args) => {
                    delete_document(&client, args.id)?;
                    OutputEnvelope {
                        mode: output_name(output).to_string(),
                        command: "documents delete".to_string(),
                        data: serde_json::json!({
                            "doc_id": args.id,
                            "status": "deleted",
                        }),
                        security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                    }
                }
                DocumentsCommand::Search(args) => {
                    session.last_query = args.query.clone();
                    let query = document_query_from_search_args(&args);
                    OutputEnvelope {
                        mode: output_name(output).to_string(),
                        command: "documents search".to_string(),
                        data: search_documents(&client, query, args.query)?,
                        security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                    }
                }
            };
            session.push_history(envelope.command.clone());
            persist_session(paths, &session)?;
            envelope
        }
        RootCommand::Search(search_command) => {
            let config = load_config_with_override(paths, overrides)?;
            let client = client_for_config(&config)?;
            match search_command {
                SearchCommand::Query(args) => OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "search query".to_string(),
                    data: query_search(&client, args.query, args.page_size, args.page)?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                },
                SearchCommand::Autocomplete(args) => OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "search autocomplete".to_string(),
                    data: autocomplete_search(&client, args.term, args.limit)?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                },
            }
        }
        RootCommand::Tasks(tasks_command) => {
            let config = load_config_with_override(paths, overrides)?;
            let client = client_for_config(&config)?;
            match tasks_command {
                TasksCommand::List => OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "tasks list".to_string(),
                    data: list_tasks(&client)?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                },
                TasksCommand::Get(args) => OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "tasks get".to_string(),
                    data: get_task(&client, args.id)?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                },
            }
        }
        RootCommand::Tags(tag_command) => {
            let config = load_config_with_override(paths, overrides)?;
            let client = client_for_config(&config)?;
            match tag_command {
                TagsCommand::List => OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "tags list".to_string(),
                    data: list_tags(&client)?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                },
                TagsCommand::Get(args) => OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "tags get".to_string(),
                    data: get_tag(&client, args.id)?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                },
                TagsCommand::Create(args) => OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "tags create".to_string(),
                    data: create_tag(&client, args.name, args.color, args.inbox)?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                },
                TagsCommand::Edit(args) => OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "tags edit".to_string(),
                    data: update_tag(
                        &client,
                        args.id,
                        &TagUpdateRequest {
                            name: args.name,
                            color: args.color,
                            inbox: args.inbox,
                        },
                    )?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                },
                TagsCommand::Delete(args) => {
                    delete_tag(&client, args.id)?;
                    OutputEnvelope {
                        mode: output_name(output).to_string(),
                        command: "tags delete".to_string(),
                        data: serde_json::json!({
                            "tag_id": args.id,
                            "status": "deleted",
                        }),
                        security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                    }
                }
            }
        }
        RootCommand::Correspondents(command) => {
            let config = load_config_with_override(paths, overrides)?;
            let client = client_for_config(&config)?;
            match command {
                CorrespondentsCommand::List => OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "correspondents list".to_string(),
                    data: list_correspondents(&client)?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                },
                CorrespondentsCommand::Get(args) => OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "correspondents get".to_string(),
                    data: get_correspondent(&client, args.id)?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                },
                CorrespondentsCommand::Create(args) => OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "correspondents create".to_string(),
                    data: create_correspondent(&client, args.name, args.matcher)?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                },
                CorrespondentsCommand::Delete(args) => {
                    delete_correspondent(&client, args.id)?;
                    OutputEnvelope {
                        mode: output_name(output).to_string(),
                        command: "correspondents delete".to_string(),
                        data: serde_json::json!({
                            "correspondent_id": args.id,
                            "status": "deleted",
                        }),
                        security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                    }
                }
            }
        }
        RootCommand::DocumentTypes(command) => {
            let config = load_config_with_override(paths, overrides)?;
            let client = client_for_config(&config)?;
            match command {
                DocumentTypesCommand::List => OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "document-types list".to_string(),
                    data: list_document_types(&client)?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                },
                DocumentTypesCommand::Get(args) => OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "document-types get".to_string(),
                    data: get_document_type(&client, args.id)?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                },
                DocumentTypesCommand::Create(args) => OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "document-types create".to_string(),
                    data: create_document_type(&client, args.name, args.matcher)?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                },
                DocumentTypesCommand::Delete(args) => {
                    delete_document_type(&client, args.id)?;
                    OutputEnvelope {
                        mode: output_name(output).to_string(),
                        command: "document-types delete".to_string(),
                        data: serde_json::json!({
                            "document_type_id": args.id,
                            "status": "deleted",
                        }),
                        security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                    }
                }
            }
        }
        RootCommand::Export(ExportCommand::Bulk(args)) => {
            let config = load_config_with_override(paths, overrides)?;
            let client = client_for_config(&config)?;
            let data = if args.zip {
                let archive_path = args
                    .output_dir
                    .join(format!("{}-documents.zip", sanitize_filename("paperless")));
                let saved = bulk_download_zip(&client, &args.ids, &archive_path, "both")?;
                serde_json::json!({
                    "status": "ok",
                    "path": saved,
                    "archive": true,
                })
            } else {
                bulk_download(&client, &args.ids, &args.output_dir, args.original)?
            };
            OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "export bulk".to_string(),
                data,
                security: auditor.review_once(&audit_state(paths, Some(&config), None)),
            }
        }
        RootCommand::Trash(trash_command) => {
            let config = load_config_with_override(paths, overrides)?;
            let client = client_for_config(&config)?;
            let (command, data) = run_trash(&client, trash_command)?;
            OutputEnvelope {
                mode: output_name(output).to_string(),
                command: command.to_string(),
                data,
                security: auditor.review_once(&audit_state(paths, Some(&config), None)),
            }
        }
        RootCommand::Api(args) => return run_api(args, paths, overrides),
        RootCommand::Dashboard => {
            let config = load_config_with_override(paths, overrides)?;
            let client = client_for_config(&config)?;
            OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "dashboard".to_string(),
                data: operational_overview(&client, &config)?,
                security: auditor.review_once(&audit_state(paths, Some(&config), None)),
            }
        }
        RootCommand::Status => match load_config(paths) {
            Ok(config) => {
                let config = apply_overrides(config, overrides)?;
                let client = client_for_config(&config)?;
                OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "status".to_string(),
                    data: status(&client, &config)?,
                    security: auditor.review_once(&audit_state(paths, Some(&config), None)),
                }
            }
            Err(error) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "status".to_string(),
                data: serde_json::json!({
                    "connected": false,
                    "message": error.to_string(),
                }),
                security: auditor.review_once(&audit_state(paths, None, None)),
            },
        },
    };

    let rendered = render_output(output, &envelope)?;
    if !quiet || !suppressible_command(&envelope.command, output) {
        println!("{rendered}");
    }
    Ok(())
}

fn run_demo_command(
    command: RootCommand,
    output: OutputMode,
    paths: &AppPaths,
    quiet: bool,
) -> Result<(), AppError> {
    let config = demo_config(output);
    let client = ApiClient::new(DemoTransport::new());
    let auditor = SecurityAuditor::new(
        SecurityAgentProfile::security_reviewer(),
        Duration::from_secs(60),
    );

    let envelope = match command {
        RootCommand::Login(_) => demo_notice_envelope(
            output,
            "login",
            json!({
                "status": "ok",
                "url": config.base_url,
                "token": config.masked_token(),
                "demo": true,
                "message": "Demo mode skips login and uses the built-in fixture dataset."
            }),
        ),
        RootCommand::Project(project_command) => match project_command {
            ProjectCommand::Login(_) => demo_notice_envelope(
                output,
                "login",
                json!({
                    "status": "ok",
                    "url": config.base_url,
                    "token": config.masked_token(),
                    "demo": true,
                    "message": "Demo mode skips login and uses the built-in fixture dataset."
                }),
            ),
            ProjectCommand::Info => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "project info".to_string(),
                data: connection_info(&client, &config, paths)?,
                security: Vec::new(),
            },
            ProjectCommand::Ping => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "project ping".to_string(),
                data: ping(&client, &config)?,
                security: Vec::new(),
            },
        },
        RootCommand::Config(config_command) => match config_command {
            ConfigCommand::SetUrl(args) => demo_notice_envelope(
                output,
                "config set-url",
                json!({
                    "status": "ok",
                    "url": args.value,
                    "token": config.masked_token(),
                    "demo": true,
                    "message": "Demo mode does not persist configuration."
                }),
            ),
            ConfigCommand::SetToken(_args) => demo_notice_envelope(
                output,
                "config set-token",
                json!({
                    "status": "ok",
                    "url": config.base_url,
                    "token": config.masked_token(),
                    "demo": true,
                    "message": "Demo mode does not persist configuration."
                }),
            ),
        },
        RootCommand::Pdf(pdf_command) => match pdf_command {
            PdfCommand::Read(args) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "pdf read".to_string(),
                data: serde_json::json!({
                    "path": args.path,
                    "text": read_local_pdf_text(&args.path).map_err(|error| AppError::Message(error.to_string()))?,
                }),
                security: auditor.review_once(&audit_state(paths, None, None)),
            },
            PdfCommand::Info(args) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "pdf info".to_string(),
                data: serde_json::to_value(
                    read_local_pdf_info(&args.path)
                        .map_err(|error| AppError::Message(error.to_string()))?,
                )?,
                security: auditor.review_once(&audit_state(paths, None, None)),
            },
        },
        RootCommand::Documents(documents_command) => match documents_command {
            DocumentsCommand::List(args) => {
                let query = document_query_from_list_args(&args);
                OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "documents list".to_string(),
                    data: list_documents(&client, &query)?,
                    security: Vec::new(),
                }
            }
            DocumentsCommand::Get(args) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "documents get".to_string(),
                data: get_document(&client, args.id)?,
                security: Vec::new(),
            },
            DocumentsCommand::Content(args) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "documents content".to_string(),
                data: get_document_content(&client, args.id)?,
                security: Vec::new(),
            },
            DocumentsCommand::Upload(args) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "documents upload".to_string(),
                data: upload_document(
                    &client,
                    &UploadRequest {
                        path: args.path,
                        title: args.title,
                        correspondent_id: args.correspondent_id,
                        document_type_id: args.type_id,
                        tag_ids: args.tag_ids,
                    },
                )?,
                security: Vec::new(),
            },
            DocumentsCommand::Download(args) => {
                let path = download_document(&client, args.id, &args.output_dir, args.original)?;
                OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "documents download".to_string(),
                    data: json!({ "doc_id": args.id, "path": path, "status": "ok", "demo": true }),
                    security: Vec::new(),
                }
            }
            DocumentsCommand::Preview(args) => {
                let path = download_preview(&client, args.id, &args.output_dir)?;
                OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "documents preview".to_string(),
                    data: json!({ "doc_id": args.id, "path": path, "status": "ok", "asset": "preview", "demo": true }),
                    security: Vec::new(),
                }
            }
            DocumentsCommand::Thumb(args) => {
                let path = download_thumbnail(&client, args.id, &args.output_dir)?;
                OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "documents thumb".to_string(),
                    data: json!({ "doc_id": args.id, "path": path, "status": "ok", "asset": "thumb", "demo": true }),
                    security: Vec::new(),
                }
            }
            DocumentsCommand::Edit(args) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "documents edit".to_string(),
                data: edit_document(
                    &client,
                    args.id,
                    &args.to_request(),
                    &args.add_tags,
                    &args.remove_tags,
                )?,
                security: Vec::new(),
            },
            DocumentsCommand::Update(args) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "documents update".to_string(),
                data: update_document(&client, args.id, &args.to_request())?,
                security: Vec::new(),
            },
            DocumentsCommand::Bulk(args) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "documents bulk".to_string(),
                data: bulk_edit(&client, &args.operations()?)?,
                security: Vec::new(),
            },
            DocumentsCommand::Delete(args) => {
                delete_document(&client, args.id)?;
                OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "documents delete".to_string(),
                    data: json!({ "doc_id": args.id, "status": "deleted", "demo": true }),
                    security: Vec::new(),
                }
            }
            DocumentsCommand::Search(args) => {
                let query = document_query_from_search_args(&args);
                OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "documents search".to_string(),
                    data: search_documents(&client, query, args.query)?,
                    security: Vec::new(),
                }
            }
        },
        RootCommand::Search(search_command) => match search_command {
            SearchCommand::Query(args) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "search query".to_string(),
                data: query_search(&client, args.query, args.page_size, args.page)?,
                security: Vec::new(),
            },
            SearchCommand::Autocomplete(args) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "search autocomplete".to_string(),
                data: autocomplete_search(&client, args.term, args.limit)?,
                security: Vec::new(),
            },
        },
        RootCommand::Tasks(tasks_command) => match tasks_command {
            TasksCommand::List => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "tasks list".to_string(),
                data: list_tasks(&client)?,
                security: Vec::new(),
            },
            TasksCommand::Get(args) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "tasks get".to_string(),
                data: get_task(&client, args.id)?,
                security: Vec::new(),
            },
        },
        RootCommand::Tags(tag_command) => match tag_command {
            TagsCommand::List => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "tags list".to_string(),
                data: list_tags(&client)?,
                security: Vec::new(),
            },
            TagsCommand::Get(args) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "tags get".to_string(),
                data: get_tag(&client, args.id)?,
                security: Vec::new(),
            },
            TagsCommand::Create(args) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "tags create".to_string(),
                data: create_tag(&client, args.name, args.color, args.inbox)?,
                security: Vec::new(),
            },
            TagsCommand::Edit(args) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "tags edit".to_string(),
                data: update_tag(
                    &client,
                    args.id,
                    &TagUpdateRequest {
                        name: args.name,
                        color: args.color,
                        inbox: args.inbox,
                    },
                )?,
                security: Vec::new(),
            },
            TagsCommand::Delete(args) => {
                delete_tag(&client, args.id)?;
                OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "tags delete".to_string(),
                    data: json!({ "tag_id": args.id, "status": "deleted", "demo": true }),
                    security: Vec::new(),
                }
            }
        },
        RootCommand::Correspondents(command) => match command {
            CorrespondentsCommand::List => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "correspondents list".to_string(),
                data: list_correspondents(&client)?,
                security: Vec::new(),
            },
            CorrespondentsCommand::Get(args) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "correspondents get".to_string(),
                data: get_correspondent(&client, args.id)?,
                security: Vec::new(),
            },
            CorrespondentsCommand::Create(args) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "correspondents create".to_string(),
                data: create_correspondent(&client, args.name, args.matcher)?,
                security: Vec::new(),
            },
            CorrespondentsCommand::Delete(args) => {
                delete_correspondent(&client, args.id)?;
                OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "correspondents delete".to_string(),
                    data: json!({ "correspondent_id": args.id, "status": "deleted", "demo": true }),
                    security: Vec::new(),
                }
            }
        },
        RootCommand::DocumentTypes(command) => match command {
            DocumentTypesCommand::List => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "document-types list".to_string(),
                data: list_document_types(&client)?,
                security: Vec::new(),
            },
            DocumentTypesCommand::Get(args) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "document-types get".to_string(),
                data: get_document_type(&client, args.id)?,
                security: Vec::new(),
            },
            DocumentTypesCommand::Create(args) => OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "document-types create".to_string(),
                data: create_document_type(&client, args.name, args.matcher)?,
                security: Vec::new(),
            },
            DocumentTypesCommand::Delete(args) => {
                delete_document_type(&client, args.id)?;
                OutputEnvelope {
                    mode: output_name(output).to_string(),
                    command: "document-types delete".to_string(),
                    data: json!({ "document_type_id": args.id, "status": "deleted", "demo": true }),
                    security: Vec::new(),
                }
            }
        },
        RootCommand::Export(ExportCommand::Bulk(args)) => {
            let data = if args.zip {
                let archive_path = args.output_dir.join(format!(
                    "{}-documents.zip",
                    sanitize_filename("paperless-demo")
                ));
                let saved = bulk_download_zip(&client, &args.ids, &archive_path, "both")?;
                json!({ "status": "ok", "path": saved, "archive": true, "demo": true })
            } else {
                bulk_download(&client, &args.ids, &args.output_dir, args.original)?
            };
            OutputEnvelope {
                mode: output_name(output).to_string(),
                command: "export bulk".to_string(),
                data,
                security: Vec::new(),
            }
        }
        RootCommand::Trash(trash_command) => {
            let (command, data) = run_trash(&client, trash_command)?;
            OutputEnvelope {
                mode: output_name(output).to_string(),
                command: command.to_string(),
                data,
                security: Vec::new(),
            }
        }
        RootCommand::Api(_) => {
            return Err(AppError::Message(
                "`api` is not available in demo mode.".to_string(),
            ))
        }
        RootCommand::Dashboard => OutputEnvelope {
            mode: output_name(output).to_string(),
            command: "dashboard".to_string(),
            data: operational_overview(&client, &config)?,
            security: Vec::new(),
        },
        RootCommand::Status => OutputEnvelope {
            mode: output_name(output).to_string(),
            command: "status".to_string(),
            data: status(&client, &config)?,
            security: Vec::new(),
        },
    };

    let rendered = render_output(output, &envelope)?;
    if !quiet || !suppressible_command(&envelope.command, output) {
        println!("{rendered}");
    }
    Ok(())
}

fn demo_notice_envelope(
    output: OutputMode,
    command: &str,
    data: serde_json::Value,
) -> OutputEnvelope {
    OutputEnvelope {
        mode: output_name(output).to_string(),
        command: command.to_string(),
        data,
        security: Vec::new(),
    }
}

fn run_api(args: ApiArgs, paths: &AppPaths, overrides: Overrides) -> Result<(), AppError> {
    let config = load_config_with_override(paths, overrides)?;
    let client = client_for_config(&config)?;
    let body = args.data.as_deref().map(parse_json_data).transpose()?;
    let bytes = api_passthrough(&client, args.method, &args.path, args.query, body, args.all)?;
    io::stdout().write_all(&bytes)?;
    Ok(())
}

fn parse_json_data(data: &str) -> Result<Value, AppError> {
    let text = match data.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|error| AppError::Message(format!("Cannot read {path}: {error}")))?,
        None => data.to_string(),
    };
    Ok(serde_json::from_str(&text)?)
}

fn parse_http_method(value: &str) -> Result<HttpMethod, String> {
    match value.to_ascii_uppercase().as_str() {
        "GET" => Ok(HttpMethod::Get),
        "POST" => Ok(HttpMethod::Post),
        "PATCH" => Ok(HttpMethod::Patch),
        "PUT" => Ok(HttpMethod::Put),
        "DELETE" => Ok(HttpMethod::Delete),
        _ => Err("expected GET, POST, PATCH, PUT or DELETE".to_string()),
    }
}

fn parse_query_pair(value: &str) -> Result<(String, String), String> {
    value
        .split_once('=')
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .ok_or_else(|| "expected KEY=VALUE".to_string())
}

fn client_for_config(config: &AppConfig) -> Result<ApiClient<ReqwestTransport>, AppError> {
    Ok(ApiClient::new(ReqwestTransport::new(config.clone())?))
}

fn load_config_with_override(
    paths: &AppPaths,
    overrides: Overrides,
) -> Result<AppConfig, AppError> {
    apply_overrides(load_config(paths)?, overrides)
}

#[derive(Clone, Copy)]
struct Overrides<'a> {
    url: Option<&'a str>,
    timeout: Option<u64>,
}

fn apply_overrides(config: AppConfig, overrides: Overrides) -> Result<AppConfig, AppError> {
    let timeout = overrides.timeout.or(config.timeout);
    let config = match overrides.url {
        Some(url) => AppConfig::new(url, config.token, config.preferred_output)?,
        None => config,
    };
    Ok(AppConfig { timeout, ..config })
}

fn suppressible_command(command: &str, output: OutputMode) -> bool {
    if matches!(output, OutputMode::Json | OutputMode::Tui) {
        return false;
    }

    matches!(
        command,
        "login"
            | "config set-url"
            | "config set-token"
            | "documents upload"
            | "documents download"
            | "documents preview"
            | "documents thumb"
            | "documents edit"
            | "documents update"
            | "documents bulk"
            | "documents delete"
            | "tags create"
            | "tags edit"
            | "tags delete"
            | "correspondents create"
            | "correspondents delete"
            | "document-types create"
            | "document-types delete"
            | "export bulk"
            | "trash restore"
            | "trash empty"
    )
}

fn set_config_url(paths: &AppPaths, url: String) -> Result<AppConfig, AppError> {
    let existing = load_config(paths).ok();
    let token = existing
        .as_ref()
        .map(|config| config.token.clone())
        .or_else(|| std::env::var("PAPERLESS_TOKEN").ok())
        .filter(|value| !value.trim().is_empty());
    let token = match token {
        Some(token) => token,
        None => prompt_secret("Paperless API token")?,
    };
    let preferred_output = existing
        .as_ref()
        .map(|config| config.preferred_output)
        .unwrap_or_default();
    let config = AppConfig {
        timeout: existing.and_then(|config| config.timeout),
        ..AppConfig::new(url, token, preferred_output)?
    };
    save_config(paths, &config)?;
    Ok(config)
}

fn set_config_token(paths: &AppPaths, token: String) -> Result<AppConfig, AppError> {
    let existing = load_config(paths).ok();
    let url = existing
        .as_ref()
        .map(|config| config.base_url.clone())
        .or_else(|| std::env::var("PAPERLESS_URL").ok())
        .filter(|value| !value.trim().is_empty());
    let url = match url {
        Some(url) => url,
        None => prompt_text("Paperless URL")?,
    };
    let preferred_output = existing
        .as_ref()
        .map(|config| config.preferred_output)
        .unwrap_or_default();
    let config = AppConfig {
        timeout: existing.and_then(|config| config.timeout),
        ..AppConfig::new(url, token, preferred_output)?
    };
    save_config(paths, &config)?;
    Ok(config)
}

fn login_envelope(
    args: LoginArgs,
    output: OutputMode,
    paths: &AppPaths,
    timeout: Option<u64>,
    auditor: &SecurityAuditor,
) -> Result<OutputEnvelope, AppError> {
    let login = resolve_login_args(args)?;
    let config = init_connection(
        |config| {
            let timeout = timeout.or(config.timeout);
            ReqwestTransport::new(AppConfig { timeout, ..config }).map(ApiClient::new)
        },
        paths,
        &login.url,
        login.token,
        login.username,
        login.password,
        output,
    )?;
    let findings = auditor.review_once(&audit_state(paths, Some(&config), None));
    Ok(OutputEnvelope {
        mode: output_name(output).to_string(),
        command: "login".to_string(),
        data: serde_json::json!({
            "status": "ok",
            "url": config.base_url,
            "token": config.masked_token(),
        }),
        security: findings,
    })
}

fn output_name(output: OutputMode) -> &'static str {
    match output {
        OutputMode::Json => "json",
        OutputMode::Markdown => "markdown",
        OutputMode::Tui => "tui",
    }
}

fn document_query_from_list_args(args: &DocumentListArgs) -> DocumentQuery {
    document_query_from_filters(args.query.clone(), &args.filters)
}

fn document_query_from_search_args(args: &DocumentSearchArgs) -> DocumentQuery {
    document_query_from_filters(Some(args.query.clone()), &args.filters)
}

fn document_query_from_filters(
    query: Option<String>,
    filters: &DocumentFilterArgs,
) -> DocumentQuery {
    DocumentQuery {
        query,
        tag: filters.tag.clone(),
        tag_id: filters.tag_id,
        correspondent: filters.correspondent.clone(),
        correspondent_id: filters.correspondent_id,
        document_type: filters.document_type.clone(),
        document_type_id: filters.type_id,
        created_after: filters.created_after.clone(),
        created_before: filters.created_before.clone(),
        order_by: filters.order_by.clone(),
        page_size: filters.page_size,
        page: filters.page,
    }
}

struct ResolvedLoginArgs {
    url: String,
    token: Option<String>,
    username: Option<String>,
    password: Option<String>,
}

fn resolve_login_args(args: LoginArgs) -> Result<ResolvedLoginArgs, AppError> {
    let url = match args.url {
        Some(url) => url,
        None => prompt_text("Paperless URL")?,
    };

    if let Some(token) = args.token {
        return Ok(ResolvedLoginArgs {
            url,
            token: Some(token),
            username: None,
            password: None,
        });
    }

    if args.username.is_some() || args.password.is_some() {
        let username = match args.username {
            Some(username) => username,
            None => prompt_text("Paperless username")?,
        };
        let password = match args.password {
            Some(password) => password,
            None => prompt_secret("Paperless password")?,
        };

        return Ok(ResolvedLoginArgs {
            url,
            token: None,
            username: Some(username),
            password: Some(password),
        });
    }

    let auth_method = prompt_text("Login with [token|password]")?;
    match auth_method.trim().to_ascii_lowercase().as_str() {
        "token" | "t" | "1" => Ok(ResolvedLoginArgs {
            url,
            token: Some(prompt_secret("Paperless API token")?),
            username: None,
            password: None,
        }),
        "password" | "p" | "2" | "userpass" => Ok(ResolvedLoginArgs {
            url,
            token: None,
            username: Some(prompt_text("Paperless username")?),
            password: Some(prompt_secret("Paperless password")?),
        }),
        _ => Err(AppError::Message(
            "Unsupported login method. Choose `token` or `password`.".to_string(),
        )),
    }
}

fn prompt_text(label: &str) -> Result<String, AppError> {
    ensure_interactive()?;
    let mut stdout = io::stdout();
    write!(stdout, "{label}: ")?;
    stdout.flush()?;

    let mut buffer = String::new();
    io::stdin().read_line(&mut buffer)?;
    let value = buffer.trim().to_string();
    if value.is_empty() {
        return Err(AppError::Message(format!("{label} cannot be empty.")));
    }
    Ok(value)
}

fn prompt_secret(label: &str) -> Result<String, AppError> {
    ensure_interactive()?;
    let value = rpassword::prompt_password(format!("{label}: "))?;
    let value = value.trim().to_string();
    if value.is_empty() {
        return Err(AppError::Message(format!("{label} cannot be empty.")));
    }
    Ok(value)
}

fn ensure_interactive() -> Result<(), AppError> {
    if io::stdin().is_terminal() {
        Ok(())
    } else {
        Err(AppError::Message(
            "Missing login details. Provide --url and either --token or --username/--password, or run `paperless login` interactively.".to_string(),
        ))
    }
}
