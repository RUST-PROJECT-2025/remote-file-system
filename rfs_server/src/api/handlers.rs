use actix_web::{HttpRequest, HttpResponse, error::{ErrorBadRequest, ErrorInternalServerError}, http::header, web::{self, Json}};
use serde::Deserialize;
use shared::file_entry::FileEntry;
use tokio::{fs, io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt}};
use tokio_util::codec::{BytesCodec, FramedRead};
use futures_util::stream::{StreamExt, TryStreamExt};
use std::io::SeekFrom;
use crate::api::{helpers::parse_range, models::SafePath};

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
pub async fn write_file_contents(path: SafePath, mut payload: web::Payload) -> Result<HttpResponse, actix_web::Error> {
    let full_path = path.into_inner();
    let mut file = fs::File::create(&full_path).await.map_err(|e| ErrorInternalServerError(e))?;

    while let Some(chunk) = payload.next().await {
        let bytes = chunk.map_err(|e| ErrorBadRequest(e))?;
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
    if !full_path.exists() { return Ok(HttpResponse::NotFound().finish()); }

    let parent = full_path.parent().ok_or(ErrorBadRequest("Invalid path"))?;
    let new_path = parent.join(&body.new_name);

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
}