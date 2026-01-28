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
}