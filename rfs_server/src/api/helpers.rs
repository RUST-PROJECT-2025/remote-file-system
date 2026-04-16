use actix_web::{HttpRequest, error::ErrorBadRequest, http::header};

/// Parsing dell'header Range per supportare il download parziale
pub fn parse_range(req: &HttpRequest, file_size: u64) -> Result<(u64, u64), actix_web::Error> {
    // se il file è 0 byte
    if file_size == 0 { return Ok((0, 0)); }
    
    // Se non c'è l'header Range, restituisci l'intero file
    let range_header = req.headers().get(header::RANGE);
    if range_header.is_none() {
        return Ok((0, file_size - 1));
    }

    // il range header deve essere del formato "bytes=start-end", 
    //oppure "bytes=start-" (richiesta aperta)
    let range_str = range_header.unwrap().to_str().map_err(|_| ErrorBadRequest("Invalid Range"))?;
    if !range_str.starts_with("bytes=") {
        return Err(ErrorBadRequest("Invalid Unit"));
    }

    let parts: Vec<&str> = range_str[6..].split('-').collect();
    let start = parts[0].parse::<u64>().map_err(|_| ErrorBadRequest("Invalid Start"))?;
    let end = if parts.len() > 1 && !parts[1].is_empty() {
        parts[1].parse::<u64>().unwrap_or(file_size - 1)
    } else {
        file_size - 1
    };

    Ok((start, std::cmp::min(end, file_size - 1)))
}