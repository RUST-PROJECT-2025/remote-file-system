use std::{path::PathBuf, num::NonZeroUsize, time::{SystemTime, Duration}, collections::HashMap};
use shared::file_entry::FileEntry;
use crate::api::Api;
use lru::LruCache;
use log::{info, debug, trace};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Inode(pub u64);

#[derive(Debug, Clone)]
pub struct CachedFile {
    pub file_entry: FileEntry,
    pub file_path: PathBuf,
}

#[derive(Debug)]
pub struct Cache {
    // cache dei file: dato l'Inode, restituisce il CachedFile (che contiene i metadati e il path completo)
    pub files: LruCache<Inode, CachedFile>,
    // mappa per cache delle directory: path -> (last_update, entries)
    pub dir_cache: HashMap<String, (SystemTime, Vec<FileEntry>)>,
    // TTL per la cache delle directory
    pub ttl: Duration,
    pub api: Api,
}

impl Cache {
    pub fn new(capacity: usize, ttl: Duration, server_url: String) -> Self {
        let cap = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::new(1000).unwrap());
        
        info!("Inizializzazione Cache: LRU capacity = {}, TTL = {:?}", cap.get(), ttl);

        Self { 
            files: LruCache::new(cap), 
            dir_cache: HashMap::new(),
            ttl,
            api: Api::new(server_url), 
        }
    }

    /// Restituisce la lista dei file in una directory, usando la cache se possibile, altrimenti chiamando l'API remota
    pub fn list_dir(&mut self, path: &str) -> Result<Vec<FileEntry>, std::io::Error> {
        // Controlliamo se abbiamo già la cartella in memoria
        if let Some((last_update, entries)) = self.dir_cache.get(path) {
            // Se è passato meno tempo del TTL, i ati sono buoni, rispondiamo subito con i dati in cache
            if last_update.elapsed().unwrap_or(Duration::MAX) < self.ttl {
                debug!("Cache hit per list_dir: {} ({} elementi)", path, entries.len());
                return Ok(entries.clone());
            } else {
                debug!("TTL scaduto per list_dir: {}", path);
            }
        }

        info!("Fetch remoto (Cache miss) per list_dir: {}", path);
        
        // Se non c'è, o il TTL è scaduto, chiamiamo l'API
        let entries = self.api.list_dir(path)?;
        let parent_path = PathBuf::from(path);
        
        for entry in &entries {
            let full_path = if path == "/" {
                PathBuf::from("/").join(&entry.name)
            } else {
                parent_path.join(&entry.name)
            };

            trace!("Cache LRU popolata: Inode {} -> {:?}", entry.ino, full_path);

            // salvo i file nella cache dei file, per poterli recuperare tramite Inode
            self.files.put(Inode(entry.ino), CachedFile {
                file_entry: entry.clone(),
                file_path: full_path,
            });
        }
        
        // Aggiorniamo la cache delle cartelle con il nuovo orario
        self.dir_cache.insert(path.to_string(), (SystemTime::now(), entries.clone()));
        
        Ok(entries)
    }


    /// Restituisce il CachedFile dato un Inode, se presente in cache. Se non è presente, restituisce None 
    pub fn get_file_by_ino(&mut self, ino: Inode) -> Option<CachedFile> {
        let result =self.files.get(&ino).cloned();

        if result.is_some() {
            trace!("Cache LRU hit per Inode: {}", ino.0);
        } else {
            trace!("Cache LRU miss per Inode: {}", ino.0);
        }

        result
    }
    
    /// Restituisce il CachedFile dato un Inode, senza aggiornare la posizione nella cache
    pub fn peek_file_by_ino(&self, ino: Inode) -> Option<CachedFile> {
        self.files.peek(&ino).cloned()
    }

    /// Invalida la cache di una directory specifica, forzando un ricaricamento alla prossima chiamata a list_dir()
    pub fn invalidate_dir(&mut self, path: &str) {
        // Rimuove la cartella dalla cache, forzando un ricaricamento
        // alla prossima chiamata a list_dir()
        debug!("Invalidazione manuale cache directory: {}", path);
        self.dir_cache.remove(path);
    }
}