use std::{path::PathBuf, num::NonZeroUsize, time::{SystemTime, Duration}, collections::HashMap};
use shared::file_entry::FileEntry;
use crate::api::Api;
use lru::LruCache;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Inode(pub u64);

#[derive(Debug, Clone)]
pub struct CachedFile {
    pub file_entry: FileEntry,
    pub file_path: PathBuf,
}

#[derive(Debug)]
pub struct Cache {
    pub files: LruCache<Inode, CachedFile>,
    // Memorizza il contenuto della cartella e il timestamp di quando è stata scaricata
    pub dir_cache: HashMap<String, (SystemTime, Vec<FileEntry>)>,
    pub ttl: Duration,
    pub api: Api,
}

impl Cache {
    //  TTL nel costruttore
    pub fn new(capacity: usize, ttl: Duration) -> Self {
        let cap = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::new(1000).unwrap());
        
        Self { 
            files: LruCache::new(cap), 
            dir_cache: HashMap::new(),
            ttl,
            api: Api::new() 
        }
    }

    pub fn list_dir(&mut self, path: &str) -> Result<Vec<FileEntry>, std::io::Error> {
        // Controlliamo se abbiamo già la cartella in memoria
        if let Some((last_update, entries)) = self.dir_cache.get(path) {
            // Se è passato meno tempo del TTL, rispondiamo subito con i dati in cache
            if last_update.elapsed().unwrap_or(Duration::MAX) < self.ttl {
                return Ok(entries.clone());
            }
        }

        // Se non c'è, o il TTL è scaduto, chiamiamo l'API
        let entries = self.api.list_dir(path)?;
        let parent_path = PathBuf::from(path);
        
        for entry in &entries {
            let full_path = if path == "/" {
                PathBuf::from("/").join(&entry.name)
            } else {
                parent_path.join(&entry.name)
            };
            self.files.put(Inode(entry.ino), CachedFile {
                file_entry: entry.clone(),
                file_path: full_path,
            });
        }
        
        // Aggiorniamo la cache delle cartelle con il nuovo orario
        self.dir_cache.insert(path.to_string(), (SystemTime::now(), entries.clone()));
        
        Ok(entries)
    }

    pub fn get_file_by_ino(&mut self, ino: Inode) -> Option<CachedFile> {
        self.files.get(&ino).cloned()
    }
    
    pub fn peek_file_by_ino(&self, ino: Inode) -> Option<CachedFile> {
        self.files.peek(&ino).cloned()
    }

    pub fn invalidate_dir(&mut self, path: &str) {
        // Rimuove la cartella dalla cache, forzando un ricaricamento
        // alla prossima chiamata a list_dir()
        self.dir_cache.remove(path);
    }
}