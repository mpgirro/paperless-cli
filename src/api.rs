use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::blocking::multipart::{Form, Part};
use serde_json::{json, Value};

use crate::config::AppConfig;
use crate::error::AppError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Endpoint {
    Relative(String),
    Absolute(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HttpMethod {
    Get,
    Post,
    Patch,
    Put,
    Delete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MultipartFile {
    pub field_name: String,
    pub file_name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestData {
    pub method: HttpMethod,
    pub endpoint: Endpoint,
    pub query: Vec<(String, String)>,
    pub json_body: Option<Value>,
    pub multipart_fields: Vec<(String, String)>,
    pub multipart_file: Option<MultipartFile>,
}

impl RequestData {
    pub fn new(method: HttpMethod, endpoint: impl Into<String>) -> Self {
        Self {
            method,
            endpoint: Endpoint::Relative(endpoint.into()),
            query: Vec::new(),
            json_body: None,
            multipart_fields: Vec::new(),
            multipart_file: None,
        }
    }

    pub fn absolute(method: HttpMethod, endpoint: impl Into<String>) -> Self {
        Self {
            method,
            endpoint: Endpoint::Absolute(endpoint.into()),
            query: Vec::new(),
            json_body: None,
            multipart_fields: Vec::new(),
            multipart_file: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponseData {
    pub status: u16,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

pub trait Transport {
    /// The configured server; `next` links must stay on its origin.
    fn base_url(&self) -> &str;

    fn send(&self, request: RequestData) -> Result<ResponseData, AppError>;

    /// Writes a successful response body to `dest` instead of `ResponseData::body`.
    fn send_to(
        &self,
        request: RequestData,
        dest: &mut dyn Write,
    ) -> Result<ResponseData, AppError> {
        let mut response = self.send(request)?;
        if (200..300).contains(&response.status) {
            dest.write_all(&response.body)?;
            response.body.clear();
        }
        Ok(response)
    }
}

#[derive(Clone)]
pub struct ReqwestTransport {
    config: AppConfig,
    client: reqwest::blocking::Client,
    upload_client: reqwest::blocking::Client,
    no_redirect_client: reqwest::blocking::Client,
}

impl ReqwestTransport {
    pub fn new(config: AppConfig) -> Result<Self, AppError> {
        // Blocking reqwest applies the client timeout to each wait: for the
        // response headers, then for every body read. A download that keeps
        // receiving data is therefore not cut off.
        let client = reqwest::blocking::Client::builder()
            .timeout(config.http_timeout())
            .connect_timeout(config.http_timeout())
            .build()?;
        // The wait for the response headers includes sending the body, so an
        // upload gets no client timeout; `send_upload` enforces an idle timeout.
        let upload_client = reqwest::blocking::Client::builder()
            .timeout(None)
            .connect_timeout(config.http_timeout())
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let no_redirect_client = reqwest::blocking::Client::builder()
            .timeout(config.http_timeout())
            .connect_timeout(config.http_timeout())
            .redirect(reqwest::redirect::Policy::none())
            .build()?;

        Ok(Self {
            config,
            client,
            upload_client,
            no_redirect_client,
        })
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }
}

impl ReqwestTransport {
    fn execute(&self, request: RequestData) -> Result<reqwest::blocking::Response, AppError> {
        Ok(self.builder(request, None)?.send()?)
    }

    fn builder(
        &self,
        request: RequestData,
        progress: Option<Arc<Mutex<Instant>>>,
    ) -> Result<reqwest::blocking::RequestBuilder, AppError> {
        // A followed 301/302 turns a write into a GET and loses it.
        let client = if request.multipart_file.is_some() {
            &self.upload_client
        } else if request.method == HttpMethod::Get {
            &self.client
        } else {
            &self.no_redirect_client
        };
        let url = match request.endpoint {
            Endpoint::Relative(path) => self.config.api_url(&path),
            Endpoint::Absolute(url) => url,
        };

        let mut builder = match request.method {
            HttpMethod::Get => client.get(&url),
            HttpMethod::Post => client.post(&url),
            HttpMethod::Patch => client.patch(&url),
            HttpMethod::Put => client.put(&url),
            HttpMethod::Delete => client.delete(&url),
        }
        .header("Authorization", format!("Token {}", self.config.token))
        .header("Accept", "application/json");

        if !request.query.is_empty() {
            builder = builder.query(&request.query);
        }

        if let Some(json_body) = request.json_body {
            builder = builder.json(&json_body);
        } else if let Some(file) = request.multipart_file {
            let mut form = Form::new();
            for (key, value) in request.multipart_fields {
                form = form.text(key, value);
            }

            let length = file.bytes.len() as u64;
            let reader = ProgressReader {
                inner: Cursor::new(file.bytes),
                progress: progress.unwrap_or_else(|| Arc::new(Mutex::new(Instant::now()))),
            };
            let part = Part::reader_with_length(reader, length)
                .file_name(file.file_name)
                .mime_str(&file.mime)
                .map_err(|error| AppError::Message(error.to_string()))?;
            form = form.part(file.field_name, part);
            builder = builder.multipart(form);
        }

        Ok(builder)
    }

    /// Fails once neither the request body nor the response has moved for the timeout.
    fn send_upload(&self, request: RequestData) -> Result<ResponseData, AppError> {
        let progress = Arc::new(Mutex::new(Instant::now()));
        let builder = self.builder(request, Some(progress.clone()))?;
        let Some(timeout) = self.config.http_timeout() else {
            return read_response(builder.send()?);
        };
        let (sender, receiver) = mpsc::channel();
        // On timeout the thread is abandoned; it ends with the process.
        std::thread::spawn(move || {
            let _ = sender.send(
                builder
                    .send()
                    .map_err(AppError::from)
                    .and_then(read_response),
            );
        });
        loop {
            let idle = progress
                .lock()
                .map(|last| last.elapsed())
                .unwrap_or_default();
            if idle >= timeout {
                return Err(AppError::Message(format!(
                    "Upload timed out: no progress for {} seconds.",
                    timeout.as_secs()
                )));
            }
            match receiver.recv_timeout(timeout - idle) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(AppError::Message("Upload thread failed.".to_string()))
                }
            }
        }
    }
}

struct ProgressReader {
    inner: Cursor<Vec<u8>>,
    progress: Arc<Mutex<Instant>>,
}

impl Read for ProgressReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        if let Ok(mut last) = self.progress.lock() {
            *last = Instant::now();
        }
        Ok(read)
    }
}

fn read_response(response: reqwest::blocking::Response) -> Result<ResponseData, AppError> {
    let mut data = response_head(&response);
    data.body = response.bytes()?.to_vec();
    Ok(data)
}

fn response_head(response: &reqwest::blocking::Response) -> ResponseData {
    ResponseData {
        status: response.status().as_u16(),
        url: response.url().to_string(),
        headers: response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.to_string().to_lowercase(),
                    value.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect(),
        body: Vec::new(),
    }
}

impl Transport for ReqwestTransport {
    fn base_url(&self) -> &str {
        &self.config.base_url
    }

    fn send(&self, request: RequestData) -> Result<ResponseData, AppError> {
        if request.multipart_file.is_some() {
            return self.send_upload(request);
        }
        read_response(self.execute(request)?)
    }

    fn send_to(
        &self,
        request: RequestData,
        dest: &mut dyn Write,
    ) -> Result<ResponseData, AppError> {
        let mut response = self.execute(request)?;
        let mut data = response_head(&response);
        if response.status().is_success() {
            response.copy_to(dest)?;
        } else {
            data.body = response.bytes()?.to_vec();
        }
        Ok(data)
    }
}

pub fn fetch_token(
    base_url: &str,
    username: &str,
    password: &str,
    timeout: Option<Duration>,
) -> Result<String, AppError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout)
        .build()?;
    let url = format!("{}/api/token/", base_url.trim_end_matches('/'));
    let response = client
        .post(&url)
        .form(&[("username", username), ("password", password)])
        .send()?;
    let status = response.status().as_u16();
    let body = response.bytes()?.to_vec();
    let response = ResponseData {
        status,
        url,
        headers: BTreeMap::new(),
        body,
    };

    if response.status != 200 {
        return Err(AppError::Http {
            status: response.status,
            url: response.url,
            message: "Failed to obtain token. Check your username and password.".to_string(),
        });
    }

    let json = parse_json_body(&response)?;
    let token = json
        .get("token")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Message("Token response missing `token`".to_string()))?;

    Ok(token.to_string())
}

#[derive(Clone)]
pub struct ApiClient<T: Transport> {
    transport: T,
}

impl<T: Transport> ApiClient<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn get_json(&self, path: &str, query: Vec<(String, String)>) -> Result<Value, AppError> {
        let mut request = RequestData::new(HttpMethod::Get, path);
        request.query = query;
        let response = self.transport.send(request)?;
        self.decode_json_response(response)
    }

    pub fn get_json_absolute(&self, url: String) -> Result<Value, AppError> {
        let request = RequestData::absolute(HttpMethod::Get, url);
        let response = self.transport.send(request)?;
        self.decode_json_response(response)
    }

    pub fn download(
        &self,
        request: RequestData,
        dest: &mut dyn Write,
    ) -> Result<ResponseData, AppError> {
        let response = self.transport.send_to(request, dest)?;
        check_response(&response)?;
        Ok(response)
    }

    pub fn post_json(&self, path: &str, body: Value) -> Result<Value, AppError> {
        let mut request = RequestData::new(HttpMethod::Post, path);
        request.json_body = Some(body);
        let response = self.transport.send(request)?;
        self.decode_json_response(response)
    }

    pub fn post_multipart(
        &self,
        path: &str,
        fields: Vec<(String, String)>,
        file: MultipartFile,
    ) -> Result<Value, AppError> {
        let mut request = RequestData::new(HttpMethod::Post, path);
        request.multipart_fields = fields;
        request.multipart_file = Some(file);
        let response = self.transport.send(request)?;
        self.decode_json_response(response)
    }

    pub fn patch_json(&self, path: &str, body: Value) -> Result<Value, AppError> {
        let mut request = RequestData::new(HttpMethod::Patch, path);
        request.json_body = Some(body);
        let response = self.transport.send(request)?;
        self.decode_json_response(response)
    }

    pub fn put_json(&self, path: &str, body: Value) -> Result<Value, AppError> {
        let mut request = RequestData::new(HttpMethod::Put, path);
        request.json_body = Some(body);
        let response = self.transport.send(request)?;
        self.decode_json_response(response)
    }

    /// Like `download`, but a non-2xx error carries the raw response body.
    pub fn send_raw_to(
        &self,
        request: RequestData,
        dest: &mut dyn Write,
    ) -> Result<ResponseData, AppError> {
        let response = self.transport.send_to(request, dest)?;
        check_raw_response(&response)?;
        Ok(response)
    }

    pub fn delete(&self, path: &str) -> Result<(), AppError> {
        let request = RequestData::new(HttpMethod::Delete, path);
        let response = self.transport.send(request)?;
        check_response(&response)?;
        Ok(())
    }

    pub fn paginate(
        &self,
        path: &str,
        query: Vec<(String, String)>,
    ) -> Result<Vec<Value>, AppError> {
        self.paginate_pages(path, query, false)
    }

    /// Like `paginate`, but a non-2xx error carries the raw body and a page without `results` fails.
    pub fn paginate_raw(
        &self,
        path: &str,
        query: Vec<(String, String)>,
    ) -> Result<Vec<Value>, AppError> {
        self.paginate_pages(path, query, true)
    }

    fn paginate_pages(
        &self,
        path: &str,
        mut query: Vec<(String, String)>,
        raw: bool,
    ) -> Result<Vec<Value>, AppError> {
        let path_has_page_size = path
            .split_once('?')
            .is_some_and(|(_, params)| params.split('&').any(|p| p.starts_with("page_size=")));
        if !path_has_page_size && !query.iter().any(|(key, _)| key == "page_size") {
            query.push(("page_size".to_string(), "100".to_string()));
        }

        let base_url = self.transport.base_url();
        let mut next = Some(Endpoint::Relative(path.to_string()));
        let mut params = query;
        let mut results = Vec::new();

        while let Some(endpoint) = next.take() {
            let mut request = match endpoint {
                Endpoint::Relative(path) => RequestData::new(HttpMethod::Get, path),
                Endpoint::Absolute(url) => RequestData::absolute(HttpMethod::Get, url),
            };
            request.query = params.clone();

            let response = self.transport.send(request)?;
            if raw {
                check_raw_response(&response)?;
            } else {
                check_response(&response)?;
            }
            let current_url = response.url.clone();
            let value = parse_json_body(&response)?;
            if let Some(items) = value.as_array() {
                results.extend(items.iter().cloned());
                break;
            }

            let response_object = value.as_object().ok_or_else(|| {
                AppError::Message("Paginated response must be an object or list".to_string())
            })?;

            match response_object.get("results").and_then(Value::as_array) {
                Some(items) => results.extend(items.iter().cloned()),
                None if raw => {
                    return Err(AppError::Message(format!(
                        "Response from {current_url} has no `results` list to paginate"
                    )))
                }
                None => {}
            }

            next = response_object
                .get("next")
                .and_then(Value::as_str)
                .map(|url| validate_next_page_url(base_url, url))
                .transpose()?
                .map(Endpoint::Absolute);
            params.clear();
        }

        Ok(results)
    }

    pub fn ping(&self, base_url: &str) -> Result<Value, AppError> {
        // Try /api/status/ first — it returns rich version/component data.
        // Paperless-ngx 3.x restricts this endpoint to users with the
        // view_status permission, so fall back to a lightweight documents
        // probe when we get a 403.
        match self.get_json("status/", Vec::new()) {
            Ok(response) => Ok(json!({
                "status": "ok",
                "url": base_url,
                "response": response,
            })),
            Err(AppError::Http { status: 403, .. }) => {
                let response = self.get_json(
                    "documents/",
                    vec![("page_size".to_string(), "1".to_string())],
                )?;
                Ok(json!({
                    "status": "ok",
                    "url": base_url,
                    "response": response,
                }))
            }
            Err(error) => Err(error),
        }
    }

    fn decode_json_response(&self, response: ResponseData) -> Result<Value, AppError> {
        check_response(&response)?;
        if response.body.is_empty() || response.status == 204 {
            return Ok(json!({}));
        }
        parse_json_body(&response)
    }
}

fn validate_next_page_url(base_url: &str, next_url: &str) -> Result<String, AppError> {
    let base = reqwest::Url::parse(base_url)
        .map_err(|_| AppError::Message(format!("Invalid base URL: {base_url}")))?;
    let next = reqwest::Url::parse(next_url)
        .map_err(|_| AppError::Message(format!("Invalid pagination URL: {next_url}")))?;

    if base.host_str() != next.host_str() {
        return Err(AppError::Message(format!(
            "Refusing cross-origin pagination link: {next_url}"
        )));
    }

    if base.scheme() == next.scheme() {
        if base.port_or_known_default() != next.port_or_known_default() {
            return Err(AppError::Message(format!(
                "Refusing cross-origin pagination link: {next_url}"
            )));
        }
        return Ok(next_url.to_string());
    }

    if next.port().is_some() && base.port_or_known_default() != next.port_or_known_default() {
        return Err(AppError::Message(format!(
            "Refusing cross-origin pagination link: {next_url}"
        )));
    }

    let mut normalized = base;
    normalized.set_path(next.path());
    normalized.set_query(next.query());
    normalized.set_fragment(next.fragment());
    Ok(normalized.to_string())
}

pub fn parse_json_body(response: &ResponseData) -> Result<Value, AppError> {
    if response.body.is_empty() {
        return Ok(json!({}));
    }
    Ok(serde_json::from_slice(&response.body)?)
}

fn check_raw_response(response: &ResponseData) -> Result<(), AppError> {
    if (200..300).contains(&response.status) {
        return Ok(());
    }
    Err(AppError::Http {
        status: response.status,
        url: response.url.clone(),
        message: String::from_utf8_lossy(&response.body).into_owned(),
    })
}

pub fn check_response(response: &ResponseData) -> Result<(), AppError> {
    match response.status {
        200..=299 => Ok(()),
        401 => Err(AppError::Http {
            status: response.status,
            url: response.url.clone(),
            message: "Authentication failed. Run `paperless login`.".to_string(),
        }),
        403 => Err(AppError::Http {
            status: response.status,
            url: response.url.clone(),
            message: "Permission denied.".to_string(),
        }),
        404 => Err(AppError::Http {
            status: response.status,
            url: response.url.clone(),
            message: "Resource not found.".to_string(),
        }),
        _ => {
            let message = parse_error_message(&response.body);
            Err(AppError::Http {
                status: response.status,
                url: response.url.clone(),
                message,
            })
        }
    }
}

fn parse_error_message(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .map(|json| {
            if let Some(detail) = json.get("detail").and_then(Value::as_str) {
                return detail.to_string();
            }
            json.to_string()
        })
        .unwrap_or_else(|| String::from_utf8_lossy(body).to_string())
}

pub fn multipart_file_from_path(path: &Path) -> Result<MultipartFile, AppError> {
    if !path.exists() {
        return Err(AppError::FileMissing(path.to_path_buf()));
    }

    let bytes = std::fs::read(path)?;
    let mime = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();

    Ok(MultipartFile {
        field_name: "document".to_string(),
        file_name: path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("document.bin")
            .to_string(),
        mime,
        bytes,
    })
}
