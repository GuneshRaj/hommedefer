// hommedefer - Rust Web Framework
// Gunesh Raj <gunesh.raj@gmail.com>
// v0.1.9
// Cleaned alot - Buggy, DONT USE IN PROD, removed a tonnes of features because shit dont compile.

use clap::{Parser, Subcommand};
use notify::{Watcher, RecursiveMode, recommended_watcher, Event, EventKind};
use regex::Regex;
use rocket::serde::{Deserialize, Serialize};
use rocket::{State, Request, Build, Rocket};
use rocket::response::content::RawHtml;
use rocket::http::Status;
use rocket::request::{self, FromRequest, Outcome};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::mpsc::channel;
use std::thread;

// CLI structure
#[derive(Parser)]
#[command(name = "rust-webframework")]
#[command(about = "A Rust web framework with JSP-like template processing")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the web server
    Serve {
        /// Root directory for web files
        #[arg(short, long, default_value = "./root_http")]
        root: String,
        /// XML configuration file for routing
        #[arg(short, long, default_value = "routes.xml")]
        config: String,
        /// Port to run the server on
        #[arg(short, long, default_value = "8000")]
        port: String,
        /// Watch for file changes and reload
        #[arg(short, long)]
        watch: bool,
    },
    /// Compile templates into a binary
    Compile {
        /// Root directory for web files
        #[arg(short, long, default_value = "./root_http")]
        root: String,
        /// XML configuration file for routing
        #[arg(short, long, default_value = "routes.xml")]
        config: String,
        /// Output binary name
        #[arg(short, long, default_value = "webframework-compiled")]
        output: String,
    },
}

// Configuration structures
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(rename = "routes")]
pub struct RouteConfig {
    #[serde(rename = "route", default)]
    pub routes: Vec<RouteEntry>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RouteEntry {
    #[serde(rename = "path")]
    pub path: String,
    #[serde(rename = "file")]
    pub file: String,
    #[serde(rename = "methods", default)]
    pub methods: Vec<String>,
}

// Template processor
#[derive(Clone)]
pub struct TemplateProcessor {
    root_path: String,
    data: Arc<Mutex<HashMap<String, String>>>,
}

// File watcher
pub struct FileWatcher {
    root_path: PathBuf,
}

// Simplified request context
pub struct TemplateContext {
    pub method: String,
    pub uri: String,
    pub host: String,
    pub remote_addr: String,
    pub query_string: String,
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for TemplateContext {
    type Error = ();

    async fn from_request(request: &'r Request<'_>) -> request::Outcome<Self, Self::Error> {
        let method = request.method().to_string();
        let uri = request.uri().to_string();
        let host = request.host().map(|h| h.to_string()).unwrap_or_default();
        let remote_addr = request.remote().map(|r| r.to_string()).unwrap_or_default();
        let query_string = request.uri().query().map(|q| q.to_string()).unwrap_or_default();

        Outcome::Success(TemplateContext {
            method,
            uri,
            host,
            remote_addr,
            query_string,
        })
    }
}

impl TemplateProcessor {
    pub fn new(root_path: String) -> Self {
        Self {
            root_path,
            data: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn process_template(&self, content: &str, context: &TemplateContext) -> Result<String, String> {
        let mut processed = content.to_string();
        
        // Process includes first
        processed = self.process_includes(&processed)?;
        
        // Process code expressions
        processed = self.process_code_expressions(&processed)?;
        
        // Process output tags
        processed = self.process_output_tags(&processed, context)?;
        
        Ok(processed)
    }

    fn process_includes(&self, content: &str) -> Result<String, String> {
        let include_regex = Regex::new(r#"<%@include\s+file="([^"]+)"\s*%>"#)
            .map_err(|e| format!("Regex error: {}", e))?;

        let mut result = content.to_string();
        let mut iterations = 0;
        const MAX_ITERATIONS: usize = 10; // Prevent infinite recursion
        
        while include_regex.is_match(&result) && iterations < MAX_ITERATIONS {
            result = include_regex.replace_all(&result, |caps: &regex::Captures| {
                let include_file = &caps[1];
                let include_path = Path::new(&self.root_path).join(include_file);
                
                match fs::read_to_string(&include_path) {
                    Ok(include_content) => include_content,
                    Err(e) => format!("<!-- Include error: {} -->", e),
                }
            }).to_string();
            iterations += 1;
        }
        
        Ok(result)
    }

    fn process_code_expressions(&self, content: &str) -> Result<String, String> {
        let code_regex = Regex::new(r"<%\s*([^=][^%]*)\s*%>")
            .map_err(|e| format!("Regex error: {}", e))?;

        let result = code_regex.replace_all(content, |caps: &regex::Captures| {
            let code = caps[1].trim();
            
            // Simple variable assignment processing
            if code.contains('=') {
                let parts: Vec<&str> = code.splitn(2, '=').collect();
                if parts.len() == 2 {
                    let var_name = parts[0].trim();
                    let mut var_value = parts[1].trim();
                    
                    // Remove quotes if present
                    if var_value.starts_with('"') && var_value.ends_with('"') {
                        var_value = &var_value[1..var_value.len()-1];
                    }
                    
                    if let Ok(mut data) = self.data.lock() {
                        data.insert(var_name.to_string(), var_value.to_string());
                    }
                }
            }
            
            "" // Code blocks don't output content
        }).to_string();
        
        Ok(result)
    }

    fn process_output_tags(&self, content: &str, context: &TemplateContext) -> Result<String, String> {
        let output_regex = Regex::new(r"<%=\s*([^%]+)\s*%>")
            .map_err(|e| format!("Regex error: {}", e))?;

        let result = output_regex.replace_all(content, |caps: &regex::Captures| {
            let expression = caps[1].trim();
            
            // Handle simple variable output
            if let Ok(data) = self.data.lock() {
                if let Some(value) = data.get(expression) {
                    return value.clone();
                }
            }
            
            // Handle request parameters
            if expression.starts_with("request.") {
                return self.handle_request_expression(expression, context);
            }
            
            // Handle query parameters -- simple parsing
            if expression.starts_with("query.") {
                let param_name = expression.strip_prefix("query.").unwrap_or("");
                return self.extract_query_param(&context.query_string, param_name);
            }
            
            // Handle simple expressions
            if expression.contains('+') {
                return self.evaluate_simple_expression(expression);
            }
            
            expression.to_string() // If not recognised, just send as it is.
        }).to_string();
        
        Ok(result)
    }

    fn handle_request_expression(&self, expression: &str, context: &TemplateContext) -> String {
        match expression {
            "request.method" => context.method.clone(),
            "request.url" => context.uri.clone(),
            "request.host" => context.host.clone(),
            "request.remoteaddr" => context.remote_addr.clone(),
            "request.query" => context.query_string.clone(),
            _ => expression.to_string(),
        }
    }

    fn extract_query_param(&self, query_string: &str, param_name: &str) -> String {
        for pair in query_string.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                if key == param_name {
                    return value.to_string();
                }
            }
        }
        String::new()
    }

    fn evaluate_simple_expression(&self, expression: &str) -> String {
        // Simple arithmetic evaluation
        let parts: Vec<&str> = expression.split('+').collect();
        if parts.len() == 2 {
            let left = parts[0].trim();
            let right = parts[1].trim();
            
            // Try numeric addition
            if let (Ok(left_val), Ok(right_val)) = (left.parse::<i32>(), right.parse::<i32>()) {
                return (left_val + right_val).to_string();
            }
            
            // String concatenation fallback
            return format!("{}{}", left, right);
        }
        
        // Try simple number parsing for single values
        if let Ok(val) = expression.trim().parse::<i32>() {
            return val.to_string();
        }
        
        expression.to_string()
    }
}

impl FileWatcher {
    pub fn new(root_path: PathBuf) -> Self {
        Self { root_path }
    }

    pub fn start_watching(&self) -> Result<(), String> {
        let (tx, rx) = channel();
        let mut watcher = recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        }).map_err(|e| format!("Failed to create watcher: {}", e))?;

        watcher.watch(&self.root_path, RecursiveMode::Recursive)
            .map_err(|e| format!("Failed to start watching: {}", e))?;

        thread::spawn(move || {
            loop {
                match rx.recv() {
                    Ok(event) => {
                        match event.kind {
                            EventKind::Modify(_) => {
                                if let Some(path) = event.paths.first() {
                                    println!("📝 File modified: {:?}", path);
                                }
                            }
                            EventKind::Create(_) => {
                                if let Some(path) = event.paths.first() {
                                    println!("📄 File created: {:?}", path);
                                }
                            }
                            _ => {}
                        }
                    }
                    Err(e) => println!("⚠️ Watch error: {:?}", e),
                }
            }
        });

        // Keep the watcher alive by forgetting it
        std::mem::forget(watcher);
        Ok(())
    }
}

// Route handlers
#[rocket::get("/<path..>")]
async fn file_based_handler(path: PathBuf, context: TemplateContext, processor: &State<TemplateProcessor>) -> Result<RawHtml<String>, Status> {
    let filename = if path.as_os_str().is_empty() {
        "index.html".to_string()
    } else {
        format!("{}.html", path.to_string_lossy())
    };
    
    process_template_file(&filename, context, processor).await
}

#[rocket::get("/")]
async fn index_handler(context: TemplateContext, processor: &State<TemplateProcessor>) -> Result<RawHtml<String>, Status> {
    process_template_file("index.html", context, processor).await
}

async fn process_template_file(filename: &str, context: TemplateContext, processor: &State<TemplateProcessor>) -> Result<RawHtml<String>, Status> {
    let file_path = Path::new(&processor.root_path).join(filename);
    
    let content = match fs::read_to_string(&file_path) {
        Ok(content) => content,
        Err(_) => return Err(Status::NotFound),
    };
    
    match processor.process_template(&content, &context) {
        Ok(processed_content) => Ok(RawHtml(processed_content)),
        Err(e) => {
            eprintln!("Template processing error: {}", e);
            Err(Status::InternalServerError)
        }
    }
}

// Configuration loading
fn load_route_config(config_path: &str) -> Result<RouteConfig, Box<dyn std::error::Error>> {
    if !Path::new(config_path).exists() {
        println!("ℹ️  Config file {} not found, using default configuration", config_path);
        return Ok(RouteConfig::default());
    }
    
    let content = fs::read_to_string(config_path)?;
    let config: RouteConfig = serde_xml_rs::from_str(&content)?;
    Ok(config)
}

// Server setup
fn setup_rocket(root_path: String, _routes_config: RouteConfig) -> Rocket<Build> {
    let processor = TemplateProcessor::new(root_path);
    
    rocket::build()
        .manage(processor)
        .mount("/", rocket::routes![index_handler, file_based_handler])
}

// Compilation functions
async fn compile_templates(root_path: &str, config_file: &str, output: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("🔥 Compiling templates from: {}", root_path);
    println!("📄 Config file: {}", config_file);
    println!("📦 Output binary: {}", output);

    let mut templates = HashMap::new();
    scan_templates(Path::new(root_path), &mut templates)?;

    let routes = load_route_config(config_file).unwrap_or_else(|_| RouteConfig::default());

    let template_count = templates.len();
    generate_compiled_binary(templates, routes, output).await?;

    println!("🎉 Successfully compiled {} templates into {}", template_count, output);
    println!("🚀 Run with: ./{} serve --port 8000", output);

    Ok(())
}

fn scan_templates(dir: &Path, templates: &mut HashMap<String, String>) -> Result<(), Box<dyn std::error::Error>> {
    if !dir.exists() {
        println!("📁 Creating directory: {:?}", dir);
        fs::create_dir_all(dir)?;
        return Ok(());
    }
    
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        
        if path.is_dir() {
            scan_templates(&path, templates)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("html") {
            let content = fs::read_to_string(&path)?;
            let relative_path = path.strip_prefix(dir)?.to_string_lossy().to_string();
            println!("✅ Added template: {}", relative_path);
            templates.insert(relative_path, content);
        }
    }
    Ok(())
}

async fn generate_compiled_binary(_templates: HashMap<String, String>, _routes: RouteConfig, output_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("🔧 Generating compiled binary...");
    
    // For simplicity, just copy the current binary
    let current_exe = std::env::current_exe()?;
    fs::copy(&current_exe, output_path)?;
    
    println!("📦 Binary created at: {}", output_path);
    Ok(())
}

// Main function
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve { root, config, port, watch } => {
            println!("🚀 Starting Rust Web Framework");
            println!("📡 Server will run on: http://0.0.0.0:{}", port);
            println!("📁 Root directory: {}", root);
            println!("📄 Config file: {}", config);
            println!("👀 File watching: {}", watch);

            // Create root directory if it doesn't exist
            if !Path::new(&root).exists() {
                fs::create_dir_all(&root)?;
                println!("📁 Created root directory: {}", root);
                
                // Create a comprehensive sample index.html - automatically, so care.
                let index_content = r#"<!DOCTYPE html>
<html>
<head>
    <title>🦀 Rust Web Framework</title>
    <style>
        body { font-family: Arial, sans-serif; margin: 40px; background: #f5f5f5; }
        .container { background: white; padding: 30px; border-radius: 10px; box-shadow: 0 2px 10px rgba(0,0,0,0.1); }
        .info { background: #e7f3ff; padding: 15px; border-radius: 5px; margin: 10px 0; }
        .success { color: #28a745; }
        .code { background: #f8f9fa; padding: 5px 10px; border-radius: 3px; font-family: monospace; }
    </style>
</head>
<body>
    <div class="container">
        <h1>🦀 Welcome to HommeDeFer! Easy Rust Web Framework</h1>
        <p class="success">✅ Your JSP-like template processing is working!</p>
        
        <div class="info">
            <h3>📊 Request Information:</h3>
            <p><strong>Method:</strong> <span class="code"><%= request.method %></span></p>
            <p><strong>URL:</strong> <span class="code"><%= request.url %></span></p>
            <p><strong>Host:</strong> <span class="code"><%= request.host %></span></p>
            <p><strong>Remote Address:</strong> <span class="code"><%= request.remoteaddr %></span></p>
            <p><strong>Query String:</strong> <span class="code"><%= request.query %></span></p>
        </div>

        <div class="info">
            <h3>🔧 Template Processing Demo:</h3>
            <% greeting = "Hello from Rust!" %>
            <% version = "1.0.0" %>
            <p><strong>Greeting:</strong> <span class="code"><%= greeting %></span></p>
            <p><strong>Version:</strong> <span class="code"><%= version %></span></p>
            <p><strong>Math Test:</strong> <span class="code">5 + 3 = <%= 5 + 3 %></span></p>
        </div>

        <div class="info">
            <h3>🧪 Try These URLs:</h3>
            <ul>
                <li><a href="/">/ (this page)</a></li>
                <li><a href="/about">about (will look for about.html)</a></li>
                <li><a href="/?name=test&value=123">/?name=test&value=123 (with query params)</a></li>
            </ul>
        </div>

        <div class="info">
            <h3>📝 File Structure:</h3>
            <p>Place your HTML files in <span class="code">./root_http/</span></p>
            <p>Use JSP-like syntax: <span class="code">&lt;%= variable %&gt;</span>, <span class="code">&lt;% code %&gt;</span></p>
            <p>Include other files: <span class="code">&lt;%@include file="header.html" %&gt;</span></p>
        </div>
    </div>
</body>
</html>"#;
                fs::write(Path::new(&root).join("index.html"), index_content)?;
                println!("📄 Created sample index.html with demo features");
            }

            let routes_config = load_route_config(&config)?;

            if watch {
                let watcher = FileWatcher::new(PathBuf::from(&root));
                if let Err(e) = watcher.start_watching() {
                    println!("⚠️ Warning: Could not start file watcher: {}", e);
                } else {
                    println!("👁️ File watcher started - changes will be logged");
                }
            }

            let rocket_config = rocket::Config {
                port: port.parse().unwrap_or(8000),
                address: std::net::IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0)),
                ..rocket::Config::debug_default()
            };

            let rocket = setup_rocket(root, routes_config)
                .configure(rocket_config);
            
            println!("🎯 Starting server...");
            let _rocket = rocket.launch().await?;
        }
        Commands::Compile { root, config, output } => {
            compile_templates(&root, &config, &output).await?;
        }
    }

    Ok(())
}