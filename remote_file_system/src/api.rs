use reqwest::{blocking::Client, StatusCode};
use shared::file_entry::FileEntry;
use std::io::{Error, ErrorKind, Read};

#[derive(Debug, Clone)]
pub struct Api {
    pub base_url: String,
    pub client: Client,
}

impl Api {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            base_url: "http://127.0.0.1:8080/api/".to_string(),
        }
    }

    pub fn list_dir(&self, path: &str) -> Result<Vec<FileEntry>, Error> {
        let url = format!("{}list{}", self.base_url, path);
        let resp = self.client.get(&url).send().map_err(|e| Error::new(ErrorKind::Other, e.to_string()))?;
        
        if resp.status() == StatusCode::NOT_FOUND {
            return Err(Error::from(ErrorKind::NotFound));
        }
        resp.json::<Vec<FileEntry>>().map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
    }

    pub fn read_file_contents(&self, path: &str, offset: u64, size: u32) -> Result<Vec<u8>, Error> {
        let end = offset + size as u64 - 1;
        let url = format!("{}files{}", self.base_url, path);
        
        let resp = self.client.get(&url)
            .header("Range", format!("bytes={}-{}", offset, end))
            .send()
            .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))?;

        match resp.status() {
            StatusCode::OK | StatusCode::PARTIAL_CONTENT => resp.bytes().map(|b| b.to_vec()).map_err(|e| Error::new(ErrorKind::Other, e.to_string())),
            StatusCode::NOT_FOUND => Err(ErrorKind::NotFound.into()),
            _ => Err(ErrorKind::Other.into()),
        }
    }

   // Accetta un generic 'R' che implementa Read + Send (es. File)
    pub fn write_file<R: Read + Send + 'static>(&self, path: &str, data: R) -> Result<(), Error> {
        let url = format!("{}files{}", self.base_url, path);
        
        // Reqwest blocking supporta il passaggio diretto di un Reader nel Body
        let resp = self.client.put(&url)
            .body(reqwest::blocking::Body::new(data))
            .send()
            .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))?;
            
        if resp.status().is_success() { Ok(()) } else { Err(ErrorKind::Other.into()) }
    }

    pub fn create_directory(&self, path: &str) -> Result<(), Error> {
        let url = format!("{}mkdir{}", self.base_url, path);
        let resp = self.client.post(&url).send().map_err(|e| Error::new(ErrorKind::Other, e.to_string()))?;
        if resp.status().is_success() { Ok(()) } else { Err(ErrorKind::Other.into()) }
    }

    pub fn delete_file_or_directory(&self, path: &str) -> Result<(), Error> {
        let url = format!("{}files{}", self.base_url, path);
        let resp = self.client.delete(&url).send().map_err(|e| Error::new(ErrorKind::Other, e.to_string()))?;
        if resp.status().is_success() { Ok(()) } else { Err(ErrorKind::Other.into()) }
    }

    pub fn rename(&self, path: &str, new_name: &str) -> Result<(), Error> {
        let url = format!("{}files{}", self.base_url, path);
        let body = serde_json::json!({ "new_name": new_name });
        let resp = self.client.patch(&url).json(&body).send().map_err(|e| Error::new(ErrorKind::Other, e.to_string()))?;
        if resp.status().is_success() { Ok(()) } else { Err(ErrorKind::Other.into()) }
    }
}