use actix_web::dev::ServiceRequest;
use actix_web::{error, web, App, Error, HttpResponse, HttpServer, Responder};
use actix_web_httpauth::extractors::bearer::BearerAuth;
use actix_web_httpauth::middleware::HttpAuthentication;
use std::env;
use std::fs::OpenOptions;
use std::io::{BufReader, BufWriter};
use std::sync::{Arc, Mutex};

fn add(item: String) -> bool {
    let mut results: Vec<String> = {
        if let Ok(file) = OpenOptions::new().read(true).open("data.json") {
            let reader = BufReader::new(file);
            serde_json::from_reader(reader).unwrap_or_else(|_| Vec::new())
        } else {
            Vec::new()
        }
    };
    if results.contains(&item) {
        return false;
    }
    results.push(item);
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open("data.json")
        .unwrap();
    let writer = BufWriter::new(file);
    serde_json::to_writer(writer, &results).unwrap();
    true
}

fn get() -> Option<String> {
    let mut results: Vec<String> = {
        let file = OpenOptions::new().read(true).open("data.json").unwrap();
        let reader = BufReader::new(file);
        serde_json::from_reader(reader).unwrap_or_else(|_| Vec::new())
    };
    // get first item
    if results.is_empty() {
        return None;
    }
    let item = results.remove(0);

    // remove first item
    let file = OpenOptions::new().write(true).open("data.json").unwrap();
    let writer = BufWriter::new(file);
    serde_json::to_writer(writer, &results).unwrap();
    Some(item)
}

#[derive(Clone)]
struct AppState {
    lock: Arc<Mutex<()>>,
}

async fn add_to_queue(data: web::Data<AppState>, item: String) -> impl Responder {
    let _lock = data.lock.lock().unwrap();
    let Ok(private_key) = hex::decode(item.replace("0x", "")) else {
        return HttpResponse::BadRequest().body("Invalid item type");
    };
    if private_key.len() != 32 {
        return HttpResponse::BadRequest().body("Invalid item length");
    }
    if add(hex::encode(private_key)) {
        HttpResponse::Ok().body("Added to the queue")
    } else {
        HttpResponse::Ok().body("Item already in the queue")
    }
}

async fn count(data: web::Data<AppState>) -> impl Responder {
    let _lock = data.lock.lock().unwrap();
    let results: Vec<String> = {
        let file = OpenOptions::new().read(true).open("data.json").unwrap();
        let reader = BufReader::new(file);
        serde_json::from_reader(reader).unwrap_or_else(|_| Vec::new())
    };
    HttpResponse::Ok().body(results.len().to_string())
}

async fn get_from_queue(data: web::Data<AppState>) -> impl Responder {
    let _lock = data.lock.lock().unwrap();
    if let Some(item) = get() {
        HttpResponse::Ok().body(item)
    } else {
        HttpResponse::BadRequest().body("Queue is empty")
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

    // Load the queue from file or create a new one

    let app_state = AppState {
        lock: Arc::new(Mutex::new(())),
    };

    HttpServer::new(move || {
        let auth = HttpAuthentication::with_fn(validator);

        App::new()
            .app_data(web::Data::new(app_state.clone()))
            .wrap(actix_web::middleware::Logger::default())
            .wrap(auth)
            .route("/count", web::get().to(count))
            .route("/add", web::post().to(add_to_queue))
            .route("/get", web::get().to(get_from_queue))
    })
    .bind("127.0.0.1:8080")?
    .workers(1)
    .run()
    .await
}
