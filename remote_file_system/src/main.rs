use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyDirectory, ReplyEntry, ReplyOpen,
    ReplyWrite, ReplyCreate, ReplyEmpty, ReplyData, Request, TimeOrNow,
};
use libc::{EBADF, ENOENT, EIO, ENOTEMPTY};
use log::{info, error};
use std::{
    collections::HashMap, 
    ffi::OsStr, 
    time::{Duration, SystemTime}, 
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    io::{Read, Write, Seek, SeekFrom},
    process::Command,
    cmp
};
use shared::file_entry::FileEntry;
use clap::Parser;
use daemonize::Daemonize;

mod api;
mod cache;
mod file;

use crate::{cache::{Cache, Inode}, file::{OpenFlags, OpenedFile, RfsFile}};

const PREFETCH_SIZE: u32 = 5 * 1024 * 1024; // 5 MB Read Ahead

// --- CLI ARGUMENTS ---
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Percorso di mount
    #[arg(short, long)]
    mount_point: String,

    /// Disabilita la cache locale
    #[arg(long)]
    no_cache: bool,

    /// Dimensione massima cache LRU (numero di file)
    #[arg(long, default_value_t = 1000)]
    cache_capacity: usize,

    /// TTL metadati cache in secondi
    #[arg(long, default_value_t = 1)]
    ttl: u64,

    /// Esegui in background (daemon mode)
    #[arg(long)]
    daemon: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fd(pub u64);

struct FileAttrWrapper(FileAttr);
impl From<FileEntry> for FileAttrWrapper {
    fn from(val: FileEntry) -> Self {
        Self(FileAttr {
            ino: val.ino,
            size: val.size,
            blocks: (val.size + 511) / 512,
            atime: val.modified_at,
            mtime: val.modified_at,
            ctime: val.modified_at,
            crtime: val.modified_at,
            kind: if val.is_dir { FileType::Directory } else { FileType::RegularFile },
            perm: val.permissions as u16,
            nlink: 1,
            uid: 1000, gid: 1000, rdev: 0, blksize: 512, flags: 0,
        })
    }
}

struct RemoteFS {
    cache: Cache,
    rfs_files: HashMap<Inode, RfsFile>,
    last_fd: AtomicU64,
    // Configurazioni
    use_cache: bool,
    ttl: Duration,
}

impl RemoteFS {
    fn new(use_cache: bool, cache_capacity: usize, ttl_secs: u64) -> Self {
        let mut cache = Cache::new(cache_capacity);
        
        // Inizializziamo manualmente la Root (Inode 1)
        cache.files.put(Inode(1), crate::cache::CachedFile {
            file_entry: FileEntry {
                ino: 1,
                name: "/".to_string(),
                is_dir: true,
                size: 0,
                modified_at: SystemTime::now(),
                permissions: 0o755,
            },
            file_path: PathBuf::from("/"),
        });

        Self {
            cache,
            rfs_files: HashMap::new(),
            last_fd: AtomicU64::new(10),
            use_cache,
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    fn alloc_fd(&self) -> Fd { Fd(self.last_fd.fetch_add(1, Ordering::SeqCst)) }

    fn get_path_str(&mut self, ino: Inode) -> Option<String> {
        // Gestione speciale Root 
        if ino.0 == 1 { 
            return Some("/".to_string()); 
        }
        self.cache.get_file_by_ino(ino).map(|f| f.file_path.to_string_lossy().to_string())
    }
}

impl Filesystem for RemoteFS {
    // METADATA 

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        // 1. Controllo se il file è aperto in scrittura (Dirty Read da buffer locale)
        if let Some(rfs_file) = self.rfs_files.get(&Inode(ino)) {
            if rfs_file.is_dirty && rfs_file.write_buffer.is_some() {
                let mut attr = FileAttrWrapper::from(rfs_file.file_entry.clone()).0;
                // Otteniamo la dimensione reale dal file temporaneo su disco
                if let Ok(metadata) = rfs_file.write_buffer.as_ref().unwrap().as_file().metadata() {
                    attr.size = metadata.len();
                }
                attr.mtime = SystemTime::now(); 
                reply.attr(&self.ttl, &attr);
                return;
            }
        }
        
        match self.cache.get_file_by_ino(Inode(ino)) {
            Some(f) => reply.attr(&self.ttl, &FileAttrWrapper::from(f.file_entry).0),
            None => reply.error(ENOENT),
        }
    }

    fn access(&mut self, _req: &Request<'_>, _ino: u64, _mask: i32, reply: ReplyEmpty) {
        reply.ok();
    }
    
    fn setattr(
        &mut self, _req: &Request<'_>, ino: u64, _mode: Option<u32>, _uid: Option<u32>, _gid: Option<u32>,
        size: Option<u64>, _atime: Option<TimeOrNow>, _mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>, _fh: Option<u64>, _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>, _bkuptime: Option<SystemTime>, _flags: Option<u32>, reply: ReplyAttr,
    ) {
        let inode = Inode(ino);
        if let Some(new_size) = size {
            if let Some(rfs_file) = self.rfs_files.get_mut(&inode) {
                // Inizializza buffer su disco se non esiste
                if rfs_file.write_buffer.is_none() { 
                    rfs_file.write_buffer = Some(tempfile::NamedTempFile::new().unwrap()); 
                }
                // Truncate del file temporaneo
                let file = rfs_file.write_buffer.as_mut().unwrap().as_file_mut();
                if file.set_len(new_size).is_ok() {
                    rfs_file.is_dirty = true;
                    rfs_file.file_entry.size = new_size;
                }
            }
        }
        self.getattr(_req, ino, None, reply);
    }

    /*fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p, None => { reply.error(ENOENT); return; }
        };
        
        // Refresh directory
        let _ = self.cache.list_dir(&parent_path);
        
        let target_path = PathBuf::from(&parent_path).join(name.to_string_lossy().as_ref());
        
        let found = self.cache.files.iter().find(|(_, f)| f.file_path == target_path).map(|(i, f)| (*i, f.clone()));

        match found {
            Some((_, f)) => reply.entry(&self.ttl, &FileAttrWrapper::from(f.file_entry).0, 0),
            None => reply.error(ENOENT),
        }
    }*/

    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p, None => { reply.error(ENOENT); return; }
        };
        
        let target_path = PathBuf::from(&parent_path).join(name.to_string_lossy().as_ref());
        
        // Cerchiamo prima nella cache locale
        let mut found = self.cache.files.iter().find(|(_, f)| f.file_path == target_path).map(|(i, f)| (*i, f.clone()));

        // Se non c'è, ALLORA scarichiamo dal server
        if found.is_none() {
             let _ = self.cache.list_dir(&parent_path);
             found = self.cache.files.iter().find(|(_, f)| f.file_path == target_path).map(|(i, f)| (*i, f.clone()));
        }

        match found {
            Some((_, f)) => reply.entry(&self.ttl, &FileAttrWrapper::from(f.file_entry).0, 0),
            None => reply.error(ENOENT),
        }
    }

    fn readdir(&mut self, _req: &Request<'_>, ino: u64, _fh: u64, offset: i64, mut reply: ReplyDirectory) {
        let path = match self.get_path_str(Inode(ino)) {
            Some(p) => p, None => { reply.error(ENOENT); return; }
        };
        
        match self.cache.list_dir(&path) {
            Ok(entries) => {
                for (i, entry) in entries.iter().enumerate().skip(offset as usize) {
                    let kind = if entry.is_dir { FileType::Directory } else { FileType::RegularFile };
                    if reply.add(entry.ino, (i + 1) as i64, kind, &entry.name) { break; }
                }
                reply.ok();
            }
            Err(_) => reply.error(EIO),
        }
    }

    // --- CREAZIONE / RIMOZIONE ---

    fn mkdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, _mode: u32, _umask: u32, reply: ReplyEntry) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p, None => { reply.error(ENOENT); return; }
        };
        let new_path = PathBuf::from(&parent_path).join(name).to_string_lossy().to_string();
        
        match self.cache.api.create_directory(&new_path) {
            Ok(_) => {
                let _ = self.cache.list_dir(&parent_path); 
                let target_path = PathBuf::from(new_path);
                
                let found = self.cache.files.iter().find(|(_, f)| f.file_path == target_path).map(|(_, f)| f.clone());
                match found {
                    Some(f) => reply.entry(&self.ttl, &FileAttrWrapper::from(f.file_entry).0, 0),
                    None => reply.error(EIO),
                }
            },
            Err(_) => reply.error(EIO),
        }
    }

    fn mknod(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, _mode: u32, _umask: u32, _rdev: u32, reply: ReplyEntry) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p, None => { reply.error(ENOENT); return; }
        };
        let new_path = PathBuf::from(&parent_path).join(name);
        
        if self.cache.api.write_file(&new_path.to_string_lossy(), std::io::empty()).is_err() { 
            reply.error(EIO); 
            return; 
        }

        let _ = self.cache.list_dir(&parent_path);
        let found = self.cache.files.iter().find(|(_, f)| f.file_path == new_path).map(|(_, f)| f.clone());

        match found {
            Some(cached) => reply.entry(&self.ttl, &FileAttrWrapper::from(cached.file_entry).0, 0),
            None => reply.error(EIO),
        }
    }

    fn create(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, _mode: u32, _umask: u32, _flags: i32, reply: ReplyCreate) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p, None => { reply.error(ENOENT); return; }
        };
        let new_path = PathBuf::from(&parent_path).join(name);
        
        if self.cache.api.write_file(&new_path.to_string_lossy(), std::io::empty()).is_err() { 
            reply.error(EIO); return; 
        }
        
        let _ = self.cache.list_dir(&parent_path);
        let found = self.cache.files.iter().find(|(_, f)| f.file_path == new_path).map(|(_, f)| f.clone());

        match found {
            Some(cached) => {
                let fd = self.alloc_fd();
                let ino = Inode(cached.file_entry.ino);
                let mut rfs_file = RfsFile::from(cached);
                
                rfs_file.fds.insert(fd, OpenedFile { fd, ino, flags: OpenFlags::WRITE });
                rfs_file.write_buffer = Some(tempfile::NamedTempFile::new().unwrap()); 
                rfs_file.is_dirty = true;
                
                let entry_for_reply = rfs_file.file_entry.clone();
                self.rfs_files.insert(ino, rfs_file);
                
                reply.created(&self.ttl, &FileAttrWrapper::from(entry_for_reply).0, 0, fd.0, 0);
            },
            None => reply.error(EIO),
        }
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p, None => { reply.error(ENOENT); return; }
        };
        let target_path = PathBuf::from(&parent_path).join(name);
        let target_path_str = target_path.to_string_lossy().to_string();

        let found_entry = self.cache.files.iter().find(|(_, f)| f.file_path == target_path).map(|(_, f)| f.clone());

        if let Some(cached) = found_entry {
            let inode = Inode(cached.file_entry.ino);
            
            let is_open = if let Some(rfs_file) = self.rfs_files.get(&inode) {
                !rfs_file.fds.is_empty()
            } else {
                false
            };

            if is_open {
                let new_name = format!(".deleted_{}_{}", inode.0, name.to_string_lossy());
                info!("Unlink su file aperto (ino={}). Rename -> {}", inode.0, new_name);

                match self.cache.api.rename(&target_path_str, &new_name) {
                    Ok(_) => {
                        if let Some(rfs_file) = self.rfs_files.get_mut(&inode) {
                            rfs_file.unlinked = true;
                            rfs_file.file_path = PathBuf::from(&parent_path).join(&new_name);
                        }
                        let _ = self.cache.list_dir(&parent_path);
                        reply.ok();
                    },
                    Err(_) => reply.error(EIO),
                }
                return;
            }
        }

        match self.cache.api.delete_file_or_directory(&target_path_str) {
            Ok(_) => reply.ok(),
            Err(_) => reply.error(EIO),
        }
    }

    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p, None => { reply.error(ENOENT); return; }
        };
        let target = PathBuf::from(&parent_path).join(name).to_string_lossy().to_string();
        
        match self.cache.api.delete_file_or_directory(&target) {
            Ok(_) => reply.ok(),
            Err(_) => reply.error(ENOTEMPTY), 
        }
    }

    fn rename(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, _new_parent: u64, new_name: &OsStr, _flags: u32, reply: ReplyEmpty) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p, None => { reply.error(ENOENT); return; }
        };
        let old_path = PathBuf::from(&parent_path).join(name).to_string_lossy().to_string();
        match self.cache.api.rename(&old_path, &new_name.to_string_lossy()) {
            Ok(_) => reply.ok(), Err(_) => reply.error(EIO),
        }
    }

    // IO

    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        let inode = Inode(ino);
        if let Some(cached) = self.cache.get_file_by_ino(inode) {
            let fd = self.alloc_fd();
            let open_flags = OpenFlags::from_flags(flags);
            let rfs_file = self.rfs_files.entry(inode).or_insert(RfsFile::from(cached));
            rfs_file.fds.insert(fd, OpenedFile { fd, ino: inode, flags: open_flags });
            
            if open_flags.is_write() && rfs_file.write_buffer.is_none() {
                 rfs_file.write_buffer = Some(tempfile::NamedTempFile::new().unwrap());
            }
            reply.opened(fd.0, 0);
        } else {
            reply.error(ENOENT);
        }
    }

    fn read(&mut self, _req: &Request<'_>, ino: u64, _fh: u64, offset: i64, size: u32, _flags: i32, _lock: Option<u64>, reply: ReplyData) {
        let inode = Inode(ino);
        
        if let Some(rfs_file) = self.rfs_files.get_mut(&inode) {
            if rfs_file.is_dirty && rfs_file.write_buffer.is_some() {
                let temp_file = rfs_file.write_buffer.as_mut().unwrap().as_file_mut();
                if temp_file.seek(SeekFrom::Start(offset as u64)).is_err() {
                    reply.error(EIO); return;
                }
                
                let mut buf = vec![0u8; size as usize];
                match temp_file.read(&mut buf) {
                    Ok(n) => reply.data(&buf[0..n]),
                    Err(_) => reply.error(EIO),
                }
                return;
            }
        }

        let rfs_file = match self.rfs_files.get_mut(&inode) {
            Some(f) => f,
            None => {
                 if let Some(cached) = self.cache.get_file_by_ino(inode) {
                     self.rfs_files.insert(inode, RfsFile::from(cached));
                     self.rfs_files.get_mut(&inode).unwrap()
                 } else {
                     reply.error(ENOENT);
                     return;
                 }
            }
        };

        let req_start = offset as u64;
        let req_end = req_start + size as u64;
        let buf_start = rfs_file.read_buffer_offset;
        let buf_end = buf_start + rfs_file.read_buffer.len() as u64;

        if req_start >= buf_start && req_end <= buf_end {
            let slice_start = (req_start - buf_start) as usize;
            let slice_end = (req_end - buf_start) as usize;
            reply.data(&rfs_file.read_buffer[slice_start..slice_end]);
            return;
        }

        let fetch_size = cmp::max(size, PREFETCH_SIZE);
        let path = rfs_file.file_path.to_str().unwrap().to_string();

        info!("Prefetching ino={} offset={}", ino, req_start);

        match self.cache.api.read_file_contents(&path, req_start, fetch_size) {
            Ok(data) => {
                rfs_file.read_buffer = data;
                rfs_file.read_buffer_offset = req_start;
                
                let available = rfs_file.read_buffer.len();
                let slice_len = cmp::min(size as usize, available);
                reply.data(&rfs_file.read_buffer[0..slice_len]);
            },
            Err(_) => reply.error(EIO),
        }
    }

    fn write(&mut self, _req: &Request<'_>, ino: u64, _fh: u64, offset: i64, data: &[u8], _wflags: u32, _flags: i32, _lock: Option<u64>, reply: ReplyWrite) {
        if let Some(rfs_file) = self.rfs_files.get_mut(&Inode(ino)) {
            if rfs_file.write_buffer.is_none() { 
                rfs_file.write_buffer = Some(tempfile::NamedTempFile::new().unwrap()); 
            }
            
            let file = rfs_file.write_buffer.as_mut().unwrap().as_file_mut();
            
            if file.seek(SeekFrom::Start(offset as u64)).is_err() {
                reply.error(EIO); return;
            }
            if file.write_all(data).is_err() {
                reply.error(EIO); return;
            }

            rfs_file.is_dirty = true;
            if let Ok(meta) = file.metadata() {
                rfs_file.file_entry.size = meta.len();
            }
            
            reply.written(data.len() as u32);
        } else {
            reply.error(EBADF);
        }
    }

    fn flush(&mut self, _req: &Request<'_>, ino: u64, _fh: u64, _lock: u64, reply: ReplyEmpty) {
        let inode = Inode(ino);
        if let Some(rfs_file) = self.rfs_files.get_mut(&inode) {
            if rfs_file.is_dirty && rfs_file.write_buffer.is_some() {
                let temp_file = rfs_file.write_buffer.as_mut().unwrap();
                let path = rfs_file.file_path.to_string_lossy().to_string();
                
                if let Ok(reader) = temp_file.reopen() {
                    info!("Streaming flush: {}", path);
                    if self.cache.api.write_file(&path, reader).is_err() {
                        reply.error(EIO);
                        return;
                    }
                    rfs_file.is_dirty = false;
                } else {
                    reply.error(EIO);
                    return;
                }
            }
        }
        reply.ok();
    }

    fn release(&mut self, _req: &Request<'_>, ino: u64, fh: u64, _flags: i32, _lock: Option<u64>, _flush: bool, reply: ReplyEmpty) {
        let inode = Inode(ino);
        let mut should_delete = false;
        let mut delete_path = String::new();

        if let Some(rfs_file) = self.rfs_files.get_mut(&inode) {
            rfs_file.fds.remove(&Fd(fh));

            if rfs_file.fds.is_empty() {
                if !rfs_file.is_dirty {
                    rfs_file.write_buffer = None; // Chiude ed elimina tempfile
                    rfs_file.read_buffer.clear();
                }
                if rfs_file.unlinked {
                    should_delete = true;
                    delete_path = rfs_file.file_path.to_string_lossy().to_string();
                }
            }
        }

        if should_delete {
            info!("Chiusura finale file unlinked. Cancellazione fisica: {}", delete_path);
            // Nota: il file potrebbe essere già stato cancellato da unlink(), quindi 
            // ignoriamo errori - il file è comunque rimosso logicamente.
            match self.cache.api.delete_file_or_directory(&delete_path) {
                Ok(_) => info!("File deleted successfully during release"),
                Err(e) => info!("File already deleted or error during release: {}", e),
            }
            // Rimuoviamo definitivamente dalla mappa dei file aperti
            self.rfs_files.remove(&inode);
        }

        reply.ok();
    }
}

fn main() {
    //env_logger::builder().filter_level(log::LevelFilter::Info).init();
    env_logger::builder().filter_level(log::LevelFilter::Debug).init();

    let args = Args::parse();
    let mountpoint = args.mount_point.clone();

    if args.daemon {
        // salvo i log
        let log_file = std::fs::File::create("/tmp/rfs.log").unwrap();
        match Daemonize::new()
            .pid_file("/tmp/rfs.pid")
            .stdout(log_file.try_clone().unwrap())
            .stderr(log_file)
            .start() {
                Ok(_) => info!("Daemon avviato."),
                Err(e) => {
                    eprintln!("Errore daemonize: {}", e);
                    return;
                }
            }
    }

    //let mp_clone = mountpoint.clone();
    /*ctrlc::set_handler(move || {
        info!("Ricevuto segnale stop. Unmounting...");
        let _ = Command::new("fusermount").arg("-u").arg(&mp_clone).status();
        std::process::exit(0);
    }).expect("Errore handler Ctrl-C");*/

    let fs = RemoteFS::new(!args.no_cache, args.cache_capacity, args.ttl);
    
    if !std::path::Path::new(&mountpoint).exists() {
        std::fs::create_dir_all(&mountpoint).unwrap();
    }
    
    info!("Mounting at {} (Cache: {}, TTL: {}s)", mountpoint, !args.no_cache, args.ttl);

    if let Err(e) = fuser::mount2(fs, &mountpoint, &[
        MountOption::FSName("remote_fs".to_string()), 
        MountOption::AutoUnmount, 
        MountOption::AllowOther
    ]) {
        error!("Errore mount: {}", e);
    }
    info!("File system smontato in modo pulito. Terminazione del demone.");
}