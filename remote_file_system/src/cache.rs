/*
use std::{collections::HashMap, path::PathBuf, time::SystemTime};
use shared::file_entry::FileEntry;
use crate::api::Api;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Inode(pub u64);

#[derive(Debug, Clone)]
pub struct CachedFile {
    pub file_entry: FileEntry,
    pub file_path: PathBuf,
}

#[derive(Debug)]
pub struct Cache {
    pub files: HashMap<Inode, CachedFile>,
    pub api: Api,
}

impl Cache {
    pub fn new() -> Self {
        let root = CachedFile {
            file_entry: FileEntry {
                ino: 1, name: "".to_string(), is_dir: true, size: 0, 
                modified_at: SystemTime::UNIX_EPOCH, permissions: 0o755 
            },
            file_path: PathBuf::from("/"),
        };
        let mut files = HashMap::new();
        files.insert(Inode(1), root);
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
            self.files.insert(Inode(entry.ino), CachedFile {
                file_entry: entry.clone(),
                file_path: full_path,
            });
        }
        Ok(entries)
    }

    pub fn get_file_by_ino(&self, ino: Inode) -> Option<CachedFile> {
        self.files.get(&ino).cloned()
    }
}*/

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};
use std::sync::RwLock;
use shared::file_entry::FileEntry;
use crate::api::Api;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Inode(pub u64);

#[derive(Debug, Clone)]
pub struct MetadataEntry {
    pub file_entry: FileEntry,
    pub file_path: PathBuf,
    pub expires_at: Instant,
}

#[derive(Debug)]
pub struct Cache {
    pub metadata: RwLock<HashMap<Inode, MetadataEntry>>,
    pub api: Api,
    pub ttl: Duration,
}

impl Cache {
    pub fn new(ttl_seconds: u64) -> Self {
        let ttl = Duration::from_secs(ttl_seconds);
        
        let mut initial_map = HashMap::new();
        
        // root
        initial_map.insert(Inode(1), MetadataEntry {
            file_entry: FileEntry {
                ino: 1,
                name: "".to_string(),
                is_dir: true,
                size: 0,
                modified_at: SystemTime::UNIX_EPOCH,
                permissions: 0o755,
            },
            file_path: PathBuf::from("/"),
            expires_at: Instant::now() + Duration::from_secs(3600 * 24),
        });

        Self {
            metadata: RwLock::new(initial_map),
            api: Api::new(),
            ttl,
        }
    }

    pub fn get_metadata(&self, ino: u64) -> Option<FileEntry> {
        let cache = self.metadata.read().unwrap();
        if let Some(entry) = cache.get(&Inode(ino)) {
            if Instant::now() < entry.expires_at {
                return Some(entry.file_entry.clone());
            }
        }
        None
    }

    pub fn list_dir(&self, path: &str) -> Result<Vec<FileEntry>, std::io::Error> {
        let entries = self.api.list_dir(path)?;
        
        let parent_path = PathBuf::from(path);
        let expiry = Instant::now() + self.ttl;

        let mut cache = self.metadata.write().unwrap();
        
        for entry in &entries {
            let full_path = if path == "/" {
                PathBuf::from("/").join(&entry.name)
            } else {
                parent_path.join(&entry.name)
            };

            cache.insert(Inode(entry.ino), MetadataEntry {
                file_entry: entry.clone(),
                file_path: full_path,
                expires_at: expiry,
            });
        }
        
        Ok(entries)
    }

    pub fn invalidate(&self, ino: u64) {
        let mut cache = self.metadata.write().unwrap();
        cache.remove(&Inode(ino));
    }

    pub fn get_file_path(&self, ino: u64) -> Option<String>{
        let cache = self.metadata.read().unwrap();
        cache.get(&Inode(ino)).map(|entry| entry.file_path.to_string_lossy().to_string())
    }
}