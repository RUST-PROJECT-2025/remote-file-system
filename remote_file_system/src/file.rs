use std::{collections::HashMap, path::PathBuf, ops::BitOr};
use shared::file_entry::FileEntry;
use crate::{cache::{CachedFile, Inode}, Fd};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenFlags(i32);

impl OpenFlags {
    pub const READ: OpenFlags = OpenFlags(1);
    pub const WRITE: OpenFlags = OpenFlags(2);
    
    pub fn from_flags(flags: i32) -> Self {
        // Logica semplificata: se c'è WRONLY o RDWR, abilita WRITE
        let acc_mode = flags & libc::O_ACCMODE;
        if acc_mode == libc::O_WRONLY || acc_mode == libc::O_RDWR {
            Self::WRITE
        } else {
            Self::READ
        }
    }
    pub fn is_write(self) -> bool { (self.0 & Self::WRITE.0) != 0 }
}

#[derive(Debug, Clone)]
pub struct OpenedFile {
    pub fd: Fd,
    pub ino: Inode,
    pub flags: OpenFlags,
}

#[derive(Debug, Clone)]
pub struct RfsFile {
    pub file_entry: FileEntry,
    pub file_path: PathBuf,
    pub fds: HashMap<Fd, OpenedFile>,
    pub write_buffer: Option<Vec<u8>>, // Buffer per scritture (dirty state)
    pub is_dirty: bool,
}

impl From<CachedFile> for RfsFile {
    fn from(c: CachedFile) -> Self {
        Self {
            file_entry: c.file_entry,
            file_path: c.file_path,
            fds: HashMap::new(),
            write_buffer: None,
            is_dirty: false,
        }
    }
}