use std::{path::PathBuf, num::NonZeroUsize};
use shared::file_entry::FileEntry;
use crate::api::Api;
use lru::LruCache; // Import necessario

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Inode(pub u64);

#[derive(Debug, Clone)]
pub struct CachedFile {
    pub file_entry: FileEntry,
    pub file_path: PathBuf,
}

#[derive(Debug)]
pub struct Cache {
    // MODIFICA: HashMap -> LruCache
    pub files: LruCache<Inode, CachedFile>,
    pub api: Api,
}

impl Cache {
    // MODIFICA: Capacity configurabile
    pub fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::new(1000).unwrap());
        
        // Inizializziamo la LRU
        let mut files = LruCache::new(cap);
        
        // Non inseriamo root manualmente qui per semplicità con LRU, 
        // verrà recuperata alla prima list_dir o lookup
        
        Self { files, api: Api::new() }
    }

    pub fn list_dir(&mut self, path: &str) -> Result<Vec<FileEntry>, std::io::Error> {
        let entries = self.api.list_dir(path)?;
        let parent_path = PathBuf::from(path);
        
        for entry in &entries {
            let full_path = if path == "/" {
                PathBuf::from("/").join(&entry.name)
            } else {
                parent_path.join(&entry.name)
            };
            // LRU put
            self.files.put(Inode(entry.ino), CachedFile {
                file_entry: entry.clone(),
                file_path: full_path,
            });
        }
        Ok(entries)
    }

    pub fn get_file_by_ino(&mut self, ino: Inode) -> Option<CachedFile> {
        // LRU get (promuove l'elemento come "usato di recente")
        self.files.get(&ino).cloned()
    }
    
    // Metodo helper per peek senza alterare l'ordine LRU (opzionale)
    pub fn peek_file_by_ino(&self, ino: Inode) -> Option<CachedFile> {
        self.files.peek(&ino).cloned()
    }
}