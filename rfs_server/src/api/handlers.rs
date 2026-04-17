use actix_web::{HttpRequest, HttpResponse, error::{ErrorBadRequest, ErrorInternalServerError}, http::header, web::{self, Json}};
use serde::Deserialize;
use shared::file_entry::FileEntry;
use tokio::{fs, io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt}};
use tokio::fs::OpenOptions;
use tokio_util::codec::{BytesCodec, FramedRead};
use futures_util::stream::{StreamExt, TryStreamExt};
use std::io::SeekFrom;
use crate::api::{helpers::parse_range, models::SafePath};
use log::info;

/// LIST
pub async fn list_directory(safe_path: SafePath) -> Result<HttpResponse, actix_web::Error> {
    let full_path = safe_path.into_inner();
    // se il path non esiste, ritorno 404
    if !full_path.exists() { return Ok(HttpResponse::NotFound().finish()); }
    
    // se il path esiste ma non è una directory, ritorno 400
    if !fs::metadata(&full_path).await?.is_dir() {
        return Err(ErrorBadRequest("Non è una directory"));
    }

    let mut entries = Vec::new();
    let mut dir = fs::read_dir(&full_path).await.map_err(|e| ErrorInternalServerError(e))?;

    while let Some(entry) = dir.next_entry().await.map_err(|e| ErrorInternalServerError(e))? {
        let metadata = entry.metadata().await.map_err(|e| ErrorInternalServerError(e))?;
        entries.push(FileEntry::from_metadata(
            entry.file_name().to_string_lossy().into_owned(),
            metadata
        ));
    }
    Ok(HttpResponse::Ok().json(entries))
}

/// READ
pub async fn read_file_contents(req: HttpRequest, path: SafePath) -> Result<HttpResponse, actix_web::Error> {
    let full_path = path.into_inner();
    if !full_path.exists() { return Ok(HttpResponse::NotFound().finish()); }

    let metadata = fs::metadata(&full_path).await?;
    let file_size = metadata.len();
    let (start, end) = parse_range(&req, file_size)?;
    let length = end - start + 1;

    let mut file = fs::File::open(&full_path).await?;
    file.seek(SeekFrom::Start(start)).await?;

    // lettura del file in streaming con FramedRead 
    let stream = FramedRead::new(file.take(length), BytesCodec::new())
        .map_ok(|bytes| web::Bytes::from(bytes.freeze()))
        .map_err(|e| ErrorInternalServerError(e));

    Ok(HttpResponse::PartialContent()
        .insert_header((header::CONTENT_RANGE, format!("bytes {}-{}/{}", start, end, file_size)))
        .insert_header((header::CONTENT_LENGTH, length))
        .streaming(stream))
}

/// WRITE (PUT)
/// scrittura di un file, con supporto a payload in streaming per file di grandi dimensioni
pub async fn write_file_contents(req: HttpRequest, path: SafePath, mut payload: web::Payload) -> Result<HttpResponse, actix_web::Error> {
    let full_path = path.into_inner();

    // leggo l'offset inviato dal client (se non c'è, default 0)
    let offset: u64 = req.headers().get("x-write-offset")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);


    info!("SERVER WRITE: Ricevuta richiesta PUT per '{:?}' con offset: {}", full_path, offset);
    
    // se il file esiste già, lo apro in modalità append e mi posiziono all'offset specificato
    let mut options = OpenOptions::new();
    options.write(true).create(true);

    // se l'offset è 0, sovrascrittura totale, quindi tronchiamo
    if offset == 0 {
        options.truncate(true);
    } 

    // se l'offset è > 0, appendo al file esistente, ma mi posiziono all'offset specificato
    let mut file = options.open(&full_path).await.map_err(|e| ErrorInternalServerError(e))?;

    // posizionamento all'offset specificato
    file.seek(SeekFrom::Start(offset)).await.map_err(|e| ErrorInternalServerError(e))?;

    while let Some(chunk) = payload.next().await {
        let bytes = chunk.map_err(|e| ErrorBadRequest(e))?;
        //info!("SERVER WRITE: Scrivendo chunk di {} bytes su '{:?}'", bytes.len(), full_path);
        file.write_all(&bytes).await.map_err(|e| ErrorInternalServerError(e))?;
    }

    Ok(HttpResponse::Created().finish())
}

/// MKDIR (POST)
pub async fn create_directory(path: SafePath) -> Result<HttpResponse, actix_web::Error> {
    let full_path = path.into_inner();
    fs::create_dir_all(&full_path).await.map_err(|e| ErrorInternalServerError(e))?;
    Ok(HttpResponse::Created().finish())
}

/// DELETE
pub async fn delete_file_or_directory(path: SafePath) -> Result<HttpResponse, actix_web::Error> {
    let full_path = path.into_inner();
    if !full_path.exists() { return Ok(HttpResponse::NotFound().finish()); }

    if fs::metadata(&full_path).await?.is_dir() {
        // la directory deve essere vuota per essere rimossa, altrimenti ritorna un errore
        fs::remove_dir(full_path).await
    } else {
        fs::remove_file(full_path).await
    }.map_err(|e| ErrorInternalServerError(e))?;
    
    Ok(HttpResponse::Ok().finish())
}

/// RENAME (PATCH)
#[derive(Deserialize)]
pub struct RenameRequest { new_name: String }

pub async fn rename_file_or_directory(path: SafePath, body: Json<RenameRequest>) -> Result<HttpResponse, actix_web::Error> {
    let full_path = path.into_inner();
    info!("SERVER RENAME: Ricevuta richiesta PATCH per '{:?}' con nuovo nome '{}'", full_path, body.new_name);
    
    if !full_path.exists() { return Ok(HttpResponse::NotFound().finish()); }


    // controllo se il nuovo nome inizia con "/" -> è una move
    let new_path = if body.new_name.starts_with('/') {
        
        // recupero la cartella radice del server 
        let storage_root = std::env::var("RFS_STORAGE_PATH")
            .unwrap_or_else(|_| "/tmp/rfs_storage".to_string());

        // tolgo lo "/" iniziale per non "rompere" il join di PathBuf
        let safe_logical_path = body.new_name.trim_start_matches('/');
        std::path::PathBuf::from(storage_root).join(safe_logical_path)

    } else {
        // È un rename
        let parent = full_path.parent().ok_or(ErrorBadRequest("Invalid path"))?;
        parent.join(&body.new_name)    
    };

    fs::rename(full_path, new_path).await.map_err(|e| ErrorInternalServerError(e))?;
    Ok(HttpResponse::Ok().finish())
}


#[cfg(test)]
mod tests {
    use actix_web::{test, web, App, http::StatusCode};
    use std::env;
    use std::fs;
    use crate::api::config_routes;

    const TEST_STORAGE: &str = "/tmp/rfs_storage_test_env";

    // Prepara un ambiente pulito per i test
    fn setup_test_env() {
        // Diciamo al server di usare questa cartella temporanea per i test
        env::set_var("RFS_STORAGE_PATH", TEST_STORAGE);
        let _ = fs::remove_dir_all(TEST_STORAGE);
        fs::create_dir_all(TEST_STORAGE).unwrap();
    }

    #[actix_web::test]
    async fn test_create_and_list_directory() {
        setup_test_env();
        let app = test::init_service(App::new().configure(config_routes)).await;

        let req_mkdir = test::TestRequest::post().uri("/api/mkdir/test_dir").to_request();
        let resp_mkdir = test::call_service(&app, req_mkdir).await;
        assert_eq!(resp_mkdir.status(), StatusCode::CREATED);

        let req_list = test::TestRequest::get().uri("/api/list/").to_request();
        let resp_list: Vec<shared::file_entry::FileEntry> = test::call_and_read_body_json(&app, req_list).await;
        assert!(resp_list.iter().any(|entry| entry.name == "test_dir" && entry.is_dir));
    }

    #[actix_web::test]
    async fn test_write_and_read_file() {
        setup_test_env();
        let app = test::init_service(App::new().configure(config_routes)).await;

        let req_put = test::TestRequest::put()
            .uri("/api/files/hello.txt")
            .set_payload("Test data")
            .to_request();
        let resp_put = test::call_service(&app, req_put).await;
        assert_eq!(resp_put.status(), StatusCode::CREATED);

        let req_get = test::TestRequest::get().uri("/api/files/hello.txt").to_request();
        let resp_body = test::call_and_read_body(&app, req_get).await;
        assert_eq!(resp_body, web::Bytes::from_static(b"Test data"));
    }

    #[actix_web::test]
    async fn test_delete_file() {
        setup_test_env();
        let app = test::init_service(App::new().configure(config_routes)).await;

        let req_put = test::TestRequest::put().uri("/api/files/todelete.txt").set_payload("data").to_request();
        test::call_service(&app, req_put).await;

        let req_del = test::TestRequest::delete().uri("/api/files/todelete.txt").to_request();
        let resp_del = test::call_service(&app, req_del).await;
        assert_eq!(resp_del.status(), StatusCode::OK);

        let req_check = test::TestRequest::get().uri("/api/files/todelete.txt").to_request();
        let resp_check = test::call_service(&app, req_check).await;
        assert_eq!(resp_check.status(), StatusCode::NOT_FOUND);
    }

    #[actix_web::test]
    async fn test_rename_file() {
        setup_test_env();
        let app = test::init_service(App::new().configure(config_routes)).await;

        // 1. Creiamo un file iniziale
        let req_put = test::TestRequest::put()
            .uri("/api/files/old_name.txt")
            .set_payload("Contenuto da rinominare")
            .to_request();
        test::call_service(&app, req_put).await;

        // 2. Chiamata PATCH per rinominarlo (inviamo solo il nuovo nome, senza '/')
        let rename_payload = serde_json::json!({ "new_name": "new_name.txt" });
        let req_rename = test::TestRequest::patch()
            .uri("/api/files/old_name.txt")
            .set_json(&rename_payload)
            .to_request();
        let resp_rename = test::call_service(&app, req_rename).await;
        assert_eq!(resp_rename.status(), StatusCode::OK);

        // 3. Verifichiamo che il vecchio file non esista più
        let req_check_old = test::TestRequest::get().uri("/api/files/old_name.txt").to_request();
        let resp_check_old = test::call_service(&app, req_check_old).await;
        assert_eq!(resp_check_old.status(), StatusCode::NOT_FOUND);

        // 4. Verifichiamo che il nuovo file esista e abbia il contenuto corretto
        let req_check_new = test::TestRequest::get().uri("/api/files/new_name.txt").to_request();
        let resp_body = test::call_and_read_body(&app, req_check_new).await;
        assert_eq!(resp_body, web::Bytes::from_static(b"Contenuto da rinominare"));
    }

    #[actix_web::test]
    async fn test_move_file_to_different_directory() {
        setup_test_env();
        let app = test::init_service(App::new().configure(config_routes)).await;

        // 1. Creiamo la directory di destinazione
        let req_mkdir = test::TestRequest::post().uri("/api/mkdir/target_folder").to_request();
        test::call_service(&app, req_mkdir).await;

        // 2. Creiamo il file originale nella root
        let req_put = test::TestRequest::put()
            .uri("/api/files/to_move.txt")
            .set_payload("Contenuto da spostare")
            .to_request();
        test::call_service(&app, req_put).await;

        // 3. Eseguiamo il MOVE inviando un percorso assoluto (inizia con '/')
        let move_payload = serde_json::json!({ "new_name": "/target_folder/moved.txt" });
        let req_move = test::TestRequest::patch()
            .uri("/api/files/to_move.txt")
            .set_json(&move_payload)
            .to_request();
        let resp_move = test::call_service(&app, req_move).await;
        assert_eq!(resp_move.status(), StatusCode::OK);

        // 4. Verifichiamo che il file originario non sia più nella root
        let req_check_old = test::TestRequest::get().uri("/api/files/to_move.txt").to_request();
        let resp_check_old = test::call_service(&app, req_check_old).await;
        assert_eq!(resp_check_old.status(), StatusCode::NOT_FOUND);

        // 5. Verifichiamo che si trovi nella nuova cartella
        let req_check_new = test::TestRequest::get().uri("/api/files/target_folder/moved.txt").to_request();
        let resp_body = test::call_and_read_body(&app, req_check_new).await;
        assert_eq!(resp_body, web::Bytes::from_static(b"Contenuto da spostare"));
    }
}