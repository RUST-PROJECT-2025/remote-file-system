use std::{collections::HashMap, path::PathBuf};
use shared::file_entry::FileEntry;
use crate::{cache::{CachedFile, Inode}, Fd};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenFlags(i32);

impl OpenFlags {
    pub const READ: OpenFlags = OpenFlags(1);
    pub const WRITE: OpenFlags = OpenFlags(2);
    
    /// Converte i flag di apertura di FUSE (O_RDONLY, O_WRONLY, O_RDWR) in OpenFlags personalizzati (WRITE o READ)
    pub fn from_flags(flags: i32) -> Self {
        let acc_mode = flags & libc::O_ACCMODE;
        if acc_mode == libc::O_WRONLY || acc_mode == libc::O_RDWR {
            Self::WRITE
        } else {
            Self::READ
        }
    }
}

#[derive(Debug, Clone)]
pub struct OpenedFile {
    pub fd: Fd,
    pub ino: Inode,
    pub flags: OpenFlags,
}


#[derive(Debug)] 
pub struct RfsFile {
    pub file_entry: FileEntry,
    pub file_path: PathBuf,
    // mappa di tutti i fd aperti su questo file, con i relativi flag e inode
    pub fds: HashMap<Fd, OpenedFile>,
    
    // tolgo il buffer di prima e tengo i dati in ram
    pub write_buffer: Vec<u8>, 

    pub is_dirty: bool,

    // serve per l'append, traccia l'offset a cui è iniziata la modifica, 
    // così da sapere da dove iniziare a scrivere quando arriva il flush
    pub write_offset: u64, 

    pub read_buffer: Vec<u8>,
    pub read_buffer_offset: u64,

    // per soft delete: se un file è stato cancellato ma è ancora aperto da qualche altra parte,
    // lo teniamo in memoria finché non viene chiuso
    pub unlinked: bool,
}

impl From<CachedFile> for RfsFile {
    fn from(c: CachedFile) -> Self {
        Self {
            file_entry: c.file_entry,
            file_path: c.file_path,
            fds: HashMap::new(),
            write_buffer: Vec::new(),
            is_dirty: false,
            write_offset: 0,
            read_buffer: Vec::new(),
            read_buffer_offset: 0,
            unlinked: false,
        }
    }
}