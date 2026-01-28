use actix_web::{HttpRequest, HttpResponse, error::{ErrorBadRequest, ErrorInternalServerError}, http::header, web::{self, Json}};
use serde::Deserialize;
use shared::file_entry::FileEntry;
use tokio::{fs, io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt}};
use tokio_util::codec::{BytesCodec, FramedRead};
use futures_util::stream::{StreamExt, TryStreamExt};
use std::io::SeekFrom;
use crate::api::{helpers::parse_range, models::SafePath};

// LIST
pub async fn list_directory(safe_path: SafePath) -> Result<HttpResponse, actix_web::Error> {
    let full_path = safe_path.into_inner();
    if !full_path.exists() { return Ok(HttpResponse::NotFound().finish()); }
    
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

// READ
pub async fn read_file_contents(req: HttpRequest, path: SafePath) -> Result<HttpResponse, actix_web::Error> {
    let full_path = path.into_inner();
    if !full_path.exists() { return Ok(HttpResponse::NotFound().finish()); }

    let metadata = fs::metadata(&full_path).await?;
    let file_size = metadata.len();
    let (start, end) = parse_range(&req, file_size)?;
    let length = end - start + 1;

    let mut file = fs::File::open(&full_path).await?;
    file.seek(SeekFrom::Start(start)).await?;

    let stream = FramedRead::new(file.take(length), BytesCodec::new())
        .map_ok(|bytes| web::Bytes::from(bytes.freeze()))
        .map_err(|e| ErrorInternalServerError(e));

    Ok(HttpResponse::PartialContent()
        .insert_header((header::CONTENT_RANGE, format!("bytes {}-{}/{}", start, end, file_size)))
        .insert_header((header::CONTENT_LENGTH, length))
        .streaming(stream))
}

// WRITE (PUT)
pub async fn write_file_contents(path: SafePath, mut payload: web::Payload) -> Result<HttpResponse, actix_web::Error> {
    let full_path = path.into_inner();
    let mut file = fs::File::create(&full_path).await.map_err(|e| ErrorInternalServerError(e))?;

    while let Some(chunk) = payload.next().await {
        let bytes = chunk.map_err(|e| ErrorBadRequest(e))?;
        file.write_all(&bytes).await.map_err(|e| ErrorInternalServerError(e))?;
    }
    Ok(HttpResponse::Created().finish())
}

// MKDIR (POST)
pub async fn create_directory(path: SafePath) -> Result<HttpResponse, actix_web::Error> {
    let full_path = path.into_inner();
    fs::create_dir_all(&full_path).await.map_err(|e| ErrorInternalServerError(e))?;
    Ok(HttpResponse::Created().finish())
}

// DELETE
pub async fn delete_file_or_directory(path: SafePath) -> Result<HttpResponse, actix_web::Error> {
    let full_path = path.into_inner();
    if !full_path.exists() { return Ok(HttpResponse::NotFound().finish()); }

    if fs::metadata(&full_path).await?.is_dir() {
        fs::remove_dir_all(full_path).await
    } else {
        fs::remove_file(full_path).await
    }.map_err(|e| ErrorInternalServerError(e))?;
    
    Ok(HttpResponse::Ok().finish())
}

// RENAME (PATCH)
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