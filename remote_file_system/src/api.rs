use reqwest::{blocking::Client, StatusCode};
use shared::file_entry::FileEntry;
use std::io::{Error, ErrorKind, Read};

#[derive(Debug, Clone)]
pub struct Api {
    pub base_url: String,
    pub client: Client,
}

impl Api {
    /// inizializza il client HTTP
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            base_url: "http://127.0.0.1:8080/api/".to_string(),
        }
    }

    /// restituisce la lista dei file in una directory, o un errore se la directory non esiste o c'è un problema di rete
    pub fn list_dir(&self, path: &str) -> Result<Vec<FileEntry>, Error> {
        // costruisce l'indirizzo hhtp per la richiesta al server
        let url = format!("{}list{}", self.base_url, path);
        let resp = self.client.get(&url).send().map_err(|e| Error::new(ErrorKind::Other, e.to_string()))?;
        
        if resp.status() == StatusCode::NOT_FOUND {
            return Err(Error::from(ErrorKind::NotFound));
        }
        resp.json::<Vec<FileEntry>>().map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
    }

    /// legge un intervallo di byte da un file specificato da path, restituendo i dati o un errore se il file non esiste o c'è un problema di rete
    pub fn read_file_contents(&self, path: &str, offset: u64, size: u32) -> Result<Vec<u8>, Error> {
        // range di bytes da leggere (offset= da dove partire, size= quanti byte leggere)
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

    /// scrive i dati in un file specificato da path, restituendo un errore se il file non esiste o c'è un problema di rete
    pub fn write_file<R: Read + Send + 'static>(&self, path: &str, data: R) -> Result<(), Error> {
        let url = format!("{}files{}", self.base_url, path);
        
        // Reqwest blocking supporta il passaggio diretto di un Reader nel Body
        // un Reader è un oggetto che implementa la lettura di dati, e può essere usato per inviare dati di grandi dimensioni senza doverli caricare tutti in ram. 
        let resp = self.client.put(&url)
            .body(reqwest::blocking::Body::new(data))
            .send()
            .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))?;
            
        if resp.status().is_success() { Ok(()) } else { Err(ErrorKind::Other.into()) }
    }

    /// crea una nuova directory specificata da path
    pub fn create_directory(&self, path: &str) -> Result<(), Error> {
        let url = format!("{}mkdir{}", self.base_url, path);
        let resp = self.client.post(&url).send().map_err(|e| Error::new(ErrorKind::Other, e.to_string()))?;
        if resp.status().is_success() { Ok(()) } else { Err(ErrorKind::Other.into()) }
    }

    /// elimina un file o una directory specificata da path
    pub fn delete_file_or_directory(&self, path: &str) -> Result<(), Error> {
        let url = format!("{}files{}", self.base_url, path);
        let resp = self.client.delete(&url).send().map_err(|e| Error::new(ErrorKind::Other, e.to_string()))?;
        if resp.status().is_success() { Ok(()) } else { Err(ErrorKind::Other.into()) }
    }

    /// rinomina un file o una directory specificata da path con un nuovo nome new_name
    pub fn rename(&self, path: &str, new_name: &str) -> Result<(), Error> {
        let url = format!("{}files{}", self.base_url, path);
        let body = serde_json::json!({ "new_name": new_name });
        let resp = self.client.patch(&url).json(&body).send().map_err(|e| Error::new(ErrorKind::Other, e.to_string()))?;
        if resp.status().is_success() { Ok(()) } else { Err(ErrorKind::Other.into()) }
    }
}