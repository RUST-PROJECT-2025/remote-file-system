use actix_web::{middleware, App, HttpServer};
use std::io;

mod api;

#[actix_web::main]
async fn main() -> io::Result<()> {
    // Inizializza il logger
    std::env::set_var("RUST_LOG", "info");
    env_logger::init();

    // Crea la cartella di storage se non esiste
    let storage_path = std::env::var("RFS_STORAGE_PATH")
        .unwrap_or_else(|_| "/tmp/rfs_storage".to_string());
    std::fs::create_dir_all(&storage_path)?;
    
    println!("Server avviato. Storage root: {}", storage_path);
    println!("Server avviato su 0.0.0.0:8080");
    //println!("In ascolto su http://127.0.0.1:8080");

    HttpServer::new(|| {
        App::new()
            .wrap(middleware::Logger::default()) // Logging delle richieste
            .configure(api::config_routes)       // Configura le rotte
    })
    //.bind(("127.0.0.1", 8080))?
    .bind(("0.0.0.0", 8080))?
    .run()
    .await
}