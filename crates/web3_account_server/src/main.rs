use actix_web::dev::ServiceRequest;
use actix_web::{error, web, App, Error, HttpResponse, HttpServer, Responder};
use actix_web_httpauth::extractors::bearer::BearerAuth;
use actix_web_httpauth::middleware::HttpAuthentication;
use std::env;
use std::fs::OpenOptions;
use std::io::{BufReader, BufWriter};
use std::sync::{Arc};
use structopt::StructOpt;

fn read_results(file_name: &str) -> Vec<String> {
    if let Ok(file) = OpenOptions::new().read(true).open(file_name) {
        let reader = BufReader::new(file);
        serde_json::from_reader(reader).unwrap_or_else(|_| Vec::new())
    } else {
        Vec::new()
    }
}

fn add(item: String, file_name: &str) -> std::io::Result<bool> {
    let mut results = read_results(file_name);
    if results.contains(&item) {
        return Ok(false);
    }
    results.push(item);
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(file_name)
        .inspect_err(|e| log::error!("Error opening file: {}", e))?;
    let writer = BufWriter::new(file);
    serde_json::to_writer(writer, &results).unwrap();
    Ok(true)
}

fn get(file_name: &str) -> std::io::Result<Option<String>> {
    let mut results = read_results(file_name);
    // get first item
    if results.is_empty() {
        return Ok(None);
    }
    let item = results.remove(0);

    // remove first item
    let file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(file_name)?;
    let writer = BufWriter::new(file);
    serde_json::to_writer(writer, &results).unwrap();
    Ok(Some(item))
}

#[derive(Clone)]
struct AppState {
    lock: Arc<tokio::sync::Mutex<()>>,
    file_name: String,
}

#[derive(Debug, StructOpt, Clone)]
pub struct CliOptions {
    #[structopt(
        long = "http-port",
        help = "Port number of the server",
        default_value = "8080"
    )]
    pub http_port: u16,

    #[structopt(
        long = "http-addr",
        help = "Bind address of the server",
        default_value = "127.0.0.1"
    )]
    pub http_addr: String,

    #[structopt(
        long = "file-name",
        help = "Name of the file to store the queue",
        default_value = "data.json"
    )]
    pub file_name: String,
}

async fn add_to_queue(data: web::Data<AppState>, item: String) -> impl Responder {
    let _lock = data.lock.lock().await;
    let Ok(private_key) = hex::decode(item.replace("0x", "")) else {
        return HttpResponse::BadRequest().body("Invalid item type");
    };
    if private_key.len() != 32 {
        return HttpResponse::BadRequest().body("Invalid item length");
    }
    match add(hex::encode(private_key), &data.file_name) {
        Ok(true) => HttpResponse::Ok().body("Added to the queue"),
        Ok(false) => HttpResponse::Ok().body("Item already in the queue"),
        Err(e) => {
            log::error!("Error adding item: {}", e);
            HttpResponse::InternalServerError().finish()
        }
    }
}

async fn count(data: web::Data<AppState>) -> impl Responder {
    let _lock = data.lock.lock().await;
    let file_name = &data.file_name;
    let results = read_results(file_name);
    HttpResponse::Ok().body(results.len().to_string())
}

async fn get_from_queue(data: web::Data<AppState>) -> impl Responder {
    let _lock = data.lock.lock().await;
    match get(&data.file_name) {
        Ok(Some(item)) => HttpResponse::Ok().body(item),
        Ok(None) => HttpResponse::BadRequest().body("Queue is empty"),
        Err(e) => {
            log::error!("Error getting item: {}", e);
            HttpResponse::InternalServerError().finish()
        }
    }
}

fn get_env_access_token() -> String {
    env::var("BEARER_KEY").unwrap_or("change_me".to_string())
}

async fn validator(
    req: ServiceRequest,
    credentials: Option<BearerAuth>,
) -> Result<ServiceRequest, (Error, ServiceRequest)> {
    if req.path() == "/count" {
        return Ok(req);
    }
    let Some(credentials) = credentials else {
        return Err((error::ErrorBadRequest("no bearer header"), req));
    };

    if credentials.token() != get_env_access_token() {
        return Err((error::ErrorBadRequest("Invalid token"), req));
    }

    Ok(req)
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env::set_var(
        "RUST_LOG",
        env::var("RUST_LOG").unwrap_or("info".to_string()),
    );
    env_logger::init();
    let args = CliOptions::from_args();
    // Load the queue from file or create a new one

    let app_state = AppState {
        lock: Arc::new(tokio::sync::Mutex::new(())),
        file_name: args.file_name,
    };

    HttpServer::new(move || {
        let auth = HttpAuthentication::with_fn(validator);

        App::new()
            .app_data(web::Data::new(app_state.clone()))
            .wrap(actix_web::middleware::Logger::default())
            .wrap(auth)
            .wrap(actix_cors::Cors::permissive())
            .route("/count", web::get().to(count))
            .route("/add", web::post().to(add_to_queue))
            .route("/get", web::get().to(get_from_queue))
    })
    .bind(format!("{}:{}", args.http_addr, args.http_port))?
    .workers(1)
    .run()
    .await
}
