use std::{collections::HashMap, path::PathBuf};
use shared::file_entry::FileEntry;
use crate::cache::{MetadataEntry, Inode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fd(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenFlags(i32);

impl OpenFlags {
    pub const READ: OpenFlags = OpenFlags(1);
    pub const WRITE: OpenFlags = OpenFlags(2);
    
    pub fn from_flags(flags: i32) -> Self {
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
    
    pub write_buffer: Option<Vec<u8>>, 
    pub is_dirty: bool,

    pub read_buffer: Vec<u8>,
    pub read_buffer_offset: u64,

    // flag per gestione unlink su file aperti
    pub unlinked: bool,
}

impl From<MetadataEntry> for RfsFile {
    fn from(md: MetadataEntry) -> Self {
        Self {
            file_entry: md.file_entry,
            file_path: md.file_path,
            fds: HashMap::new(),
            write_buffer: None,
            is_dirty: false,
            read_buffer: Vec::new(),
            read_buffer_offset: 0,
            unlinked: false,
        }
    }
}