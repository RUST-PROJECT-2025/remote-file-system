use actix_web::web;

pub mod handlers;
pub mod helpers;
pub mod models;

pub fn config_routes(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api")
            .route("/list/{tail:.*}", web::get().to(handlers::list_directory))
            .service(
                web::resource("/files/{tail:.*}")
                    .route(web::get().to(handlers::read_file_contents))
                    .route(web::put().to(handlers::write_file_contents))
                    .route(web::delete().to(handlers::delete_file_or_directory))
                    .route(web::patch().to(handlers::rename_file_or_directory)),
            )
            .route("/mkdir/{tail:.*}", web::post().to(handlers::create_directory)),
    );
}