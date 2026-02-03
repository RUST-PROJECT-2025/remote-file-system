use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyDirectory, ReplyEntry, ReplyOpen,
    ReplyWrite, ReplyCreate, ReplyEmpty, ReplyData, Request,
};
use libc::{EBADF, ENOENT, EIO, ENOSYS};
use log::{info, error, warn};
use std::{
    collections::HashMap, ffi::OsStr, time::{Duration, SystemTime}, path::PathBuf,
    sync::atomic::{AtomicU64, Ordering}
};
use shared::file_entry::FileEntry;
use std::cmp;

mod api;
mod cache;
mod file;

use crate::{cache::{Cache, Inode}, file::{OpenFlags, OpenedFile, RfsFile}};

const TTL: Duration = Duration::from_secs(1);
const PREFETCH_SIZE: u32 = 5 * 1024 * 1024; // 5 MB Read Ahead

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
}

impl RemoteFS {
    fn new() -> Self {
        Self {
            cache: Cache::new(),
            rfs_files: HashMap::new(),
            last_fd: AtomicU64::new(10),
        }
    }

    fn alloc_fd(&self) -> Fd { Fd(self.last_fd.fetch_add(1, Ordering::SeqCst)) }

    fn get_path_str(&self, ino: Inode) -> Option<String> {
        self.cache.get_file_by_ino(ino).map(|f| f.file_path.to_string_lossy().to_string())
    }
}

impl Filesystem for RemoteFS {
    // --- METADATA ---

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        if let Some(rfs_file) = self.rfs_files.get(&Inode(ino)) {
            if rfs_file.is_dirty && rfs_file.write_buffer.is_some() {
                let mut attr = FileAttrWrapper::from(rfs_file.file_entry.clone()).0;
                attr.size = rfs_file.write_buffer.as_ref().unwrap().len() as u64;
                attr.mtime = SystemTime::now(); 
                reply.attr(&TTL, &attr);
                return;
            }
        }
        
        match self.cache.get_file_by_ino(Inode(ino)) {
            Some(f) => reply.attr(&TTL, &FileAttrWrapper::from(f.file_entry).0),
            None => reply.error(ENOENT),
        }
    }

    fn setattr(
        &mut self, _req: &Request<'_>, ino: u64, mode: Option<u32>, _uid: Option<u32>, _gid: Option<u32>,
        size: Option<u64>, _atime: Option<fuser::TimeOrNow>, _mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<SystemTime>, _fh: Option<u64>, _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>, _bkuptime: Option<SystemTime>, _flags: Option<u32>, reply: ReplyAttr,
    ) {
        let inode = Inode(ino);
        if let Some(new_size) = size {
            if let Some(rfs_file) = self.rfs_files.get_mut(&inode) {
                if rfs_file.write_buffer.is_none() { rfs_file.write_buffer = Some(Vec::new()); }
                let buf = rfs_file.write_buffer.as_mut().unwrap();
                buf.resize(new_size as usize, 0);
                rfs_file.is_dirty = true;
                rfs_file.file_entry.size = new_size;
            }
        }
        self.getattr(_req, ino, None, reply);
    }

    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p, None => { reply.error(ENOENT); return; }
        };
        let _ = self.cache.list_dir(&parent_path);
        let target_path = PathBuf::from(&parent_path).join(name.to_string_lossy().as_ref());
        match self.cache.files.values().find(|f| f.file_path == target_path) {
            Some(f) => reply.entry(&TTL, &FileAttrWrapper::from(f.file_entry.clone()).0, 0),
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
                match self.cache.files.values().find(|f| f.file_path == target_path) {
                    Some(f) => reply.entry(&TTL, &FileAttrWrapper::from(f.file_entry.clone()).0, 0),
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
        
        // 1. Costruiamo il PathBuf target una volta sola
        let new_path = PathBuf::from(&parent_path).join(name);
        let new_path_str = new_path.to_string_lossy().to_string();

        // 2. Usiamo la stringa per la chiamata API
        if self.cache.api.write_file(&new_path_str, vec![]).is_err() { 
            reply.error(EIO); 
            return; 
        }

        // 3. Aggiorniamo la cache
        let _ = self.cache.list_dir(&parent_path);

        // 4. FIX: Ora confrontiamo direttamente con `new_path` (PathBuf).
        // La closure catturerà `new_path` per riferimento, senza consumarlo.
        match self.cache.files.values().find(|f| f.file_path == new_path).cloned() {
            Some(cached) => reply.entry(&TTL, &FileAttrWrapper::from(cached.file_entry).0, 0),
            None => reply.error(EIO),
        }
    }
    fn create(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, _mode: u32, _umask: u32, _flags: i32, reply: ReplyCreate) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p, None => { reply.error(ENOENT); return; }
        };
        let new_path = PathBuf::from(&parent_path).join(name);
        if self.cache.api.write_file(&new_path.to_string_lossy(), vec![]).is_err() { reply.error(EIO); return; }
        let _ = self.cache.list_dir(&parent_path);
        match self.cache.files.values().find(|f| f.file_path == new_path).cloned() {
            Some(cached) => {
                let fd = self.alloc_fd();
                let ino = Inode(cached.file_entry.ino);
                let mut rfs_file = RfsFile::from(cached);
                rfs_file.fds.insert(fd, OpenedFile { fd, ino, flags: OpenFlags::WRITE });
                rfs_file.write_buffer = Some(Vec::new()); 
                rfs_file.is_dirty = true;
                self.rfs_files.insert(ino, rfs_file.clone());
                reply.created(&TTL, &FileAttrWrapper::from(rfs_file.file_entry).0, 0, fd.0, 0);
            },
            None => reply.error(EIO),
        }
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p,
            None => { reply.error(ENOENT); return; }
        };
        let target_path = PathBuf::from(&parent_path).join(name);
        let target_path_str = target_path.to_string_lossy().to_string();

        // 1. Cerchiamo l'inode del file target per vedere se è aperto localmente
        //    (Dobbiamo cercarlo nella cache perché unlink ci da solo nome e parent)
        let found_entry = self.cache.files.values().find(|f| f.file_path == target_path).cloned();

        if let Some(cached) = found_entry {
            let inode = Inode(cached.file_entry.ino);
            
            // 2. Controllo se il file è APERTO (è presente in rfs_files e ha file descriptor attivi)
            let is_open = if let Some(rfs_file) = self.rfs_files.get(&inode) {
                !rfs_file.fds.is_empty()
            } else {
                false
            };

            if is_open {
                // CASO A: Il file è aperto. NON cancellare. Rinomina in un file nascosto.
                let new_name = format!(".deleted_{}_{}", inode.0, name.to_string_lossy());
                
                info!("Unlink su file aperto (ino={}). Rinomino in {} per preservare lettura.", inode.0, new_name);

                // Eseguiamo il rename sul server
                match self.cache.api.rename(&target_path_str, &new_name) {
                    Ok(_) => {
                        // Aggiorniamo lo stato locale
                        if let Some(rfs_file) = self.rfs_files.get_mut(&inode) {
                            rfs_file.unlinked = true;
                            // Aggiorniamo il path locale in modo che le future read/write puntino al file rinominato
                            rfs_file.file_path = PathBuf::from(&parent_path).join(&new_name);
                        }
                        
                        // Invalidiamo la cache della directory genitore così ls non mostra più il file originale
                        let _ = self.cache.list_dir(&parent_path);
                        reply.ok();
                    },
                    Err(e) => {
                        error!("Fallito rename per soft-delete: {}", e);
                        reply.error(EIO);
                    }
                }
                return;
            }
        }

        // CASO B: Il file non è aperto (o non trovato in cache locale). Cancellazione standard immediata.
        match self.cache.api.delete_file_or_directory(&target_path_str) {
            Ok(_) => reply.ok(),
            Err(_) => reply.error(EIO),
        }
    }

    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p,
            None => { reply.error(ENOENT); return; }
        };
        let target = PathBuf::from(&parent_path).join(name).to_string_lossy().to_string();
        
        // rmdir deve fallire se la directory non è vuota.
        // Se il server fallisce (perché ora usa remove_dir), ritorniamo ENOTEMPTY.
        match self.cache.api.delete_file_or_directory(&target) {
            Ok(_) => reply.ok(),
            // Nota: In un'implementazione perfetta, l'API dovrebbe ritornare 409 Conflict
            // e il client dovrebbe distinguere l'errore. Qui assumiamo che errore su rmdir = non vuota.
            Err(_) => reply.error(libc::ENOTEMPTY), 
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

    // --- IO ---

    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        let inode = Inode(ino);
        if let Some(cached) = self.cache.get_file_by_ino(inode) {
            let fd = self.alloc_fd();
            let open_flags = OpenFlags::from_flags(flags);
            let rfs_file = self.rfs_files.entry(inode).or_insert(RfsFile::from(cached));
            rfs_file.fds.insert(fd, OpenedFile { fd, ino: inode, flags: open_flags });
            if open_flags.is_write() && rfs_file.write_buffer.is_none() {
                 rfs_file.write_buffer = Some(Vec::new());
            }
            reply.opened(fd.0, 0);
        } else {
            reply.error(ENOENT);
        }
    }

    // *** LETTURA OTTIMIZZATA (BUFFERED) ***
    fn read(&mut self, _req: &Request<'_>, ino: u64, _fh: u64, offset: i64, size: u32, _flags: i32, _lock: Option<u64>, reply: ReplyData) {
        let inode = Inode(ino);
        
        // 1. Controllo Scrittura Locale (Priority)
        if let Some(rfs_file) = self.rfs_files.get(&inode) {
            if rfs_file.is_dirty && rfs_file.write_buffer.is_some() {
                let buf = rfs_file.write_buffer.as_ref().unwrap();
                let start = offset as usize;
                if start >= buf.len() { reply.data(&[]); return; }
                let end = cmp::min(start + size as usize, buf.len());
                reply.data(&buf[start..end]);
                return;
            }
        }

        // 2. Controllo Buffer di Lettura (Prefetching)
        // Dobbiamo ottenere mutable reference per aggiornare il buffer se necessario
        // Se il file non è in rfs_files (strano per una read), proviamo a ricaricarlo dalla cache
        let rfs_file = match self.rfs_files.get_mut(&inode) {
            Some(f) => f,
            None => {
                 // Fallback: load from cache metadata if not open? 
                 // FUSE garantisce che open venga chiamato prima, ma se siamo qui, gestiamo l'errore o ricarichiamo.
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

        // Cache Hit?
        if req_start >= buf_start && req_end <= buf_end {
            let slice_start = (req_start - buf_start) as usize;
            let slice_end = (req_end - buf_start) as usize;
            reply.data(&rfs_file.read_buffer[slice_start..slice_end]);
            return;
        }

        // Cache Miss: Download Chunk (Prefetching)
        let fetch_size = cmp::max(size, PREFETCH_SIZE);
        let path = rfs_file.file_path.to_str().unwrap().to_string(); // Clone path per borrow checker

        info!("Cache miss ino={}, offset={}. Prefetching {} bytes...", ino, req_start, fetch_size);

        match self.cache.api.read_file_contents(&path, req_start, fetch_size) {
            Ok(data) => {
                // Aggiorna buffer
                rfs_file.read_buffer = data;
                rfs_file.read_buffer_offset = req_start;
                
                // Servi richiesta
                let available = rfs_file.read_buffer.len();
                let slice_len = cmp::min(size as usize, available);
                reply.data(&rfs_file.read_buffer[0..slice_len]);
            },
            Err(_) => reply.error(EIO),
        }
    }

    fn write(&mut self, _req: &Request<'_>, ino: u64, _fh: u64, offset: i64, data: &[u8], _wflags: u32, _flags: i32, _lock: Option<u64>, reply: ReplyWrite) {
        if let Some(rfs_file) = self.rfs_files.get_mut(&Inode(ino)) {
            if rfs_file.write_buffer.is_none() { rfs_file.write_buffer = Some(Vec::new()); }
            let buf = rfs_file.write_buffer.as_mut().unwrap();
            let end = offset as usize + data.len();
            if end > buf.len() { buf.resize(end, 0); }
            buf[offset as usize..end].copy_from_slice(data);
            rfs_file.is_dirty = true;
            rfs_file.file_entry.size = buf.len() as u64; 
            reply.written(data.len() as u32);
        } else {
            reply.error(EBADF);
        }
    }

    fn flush(&mut self, _req: &Request<'_>, ino: u64, _fh: u64, _lock: u64, reply: ReplyEmpty) {
        let inode = Inode(ino);
        if let Some(rfs_file) = self.rfs_files.get_mut(&inode) {
            if rfs_file.is_dirty && rfs_file.write_buffer.is_some() {
                let data = rfs_file.write_buffer.as_ref().unwrap().clone();
                let path = rfs_file.file_path.to_string_lossy().to_string();
                info!("Flushing file to server: {}", path);
                if self.cache.api.write_file(&path, data).is_err() {
                    reply.error(EIO);
                    return;
                }
                rfs_file.is_dirty = false;
            }
        }
        reply.ok();
    }

    fn release(&mut self, _req: &Request<'_>, ino: u64, fh: u64, _flags: i32, _lock: Option<u64>, _flush: bool, reply: ReplyEmpty) {
        let inode = Inode(ino);
        let mut should_delete = false;
        let mut delete_path = String::new();

        if let Some(rfs_file) = self.rfs_files.get_mut(&inode) {
            // Rimuoviamo il File Descriptor
            rfs_file.fds.remove(&Fd(fh));

            // Se non ci sono più FD aperti...
            if rfs_file.fds.is_empty() {
                // Pulizia memoria buffer
                if !rfs_file.is_dirty {
                    rfs_file.write_buffer = None;
                    rfs_file.read_buffer.clear();
                }

                // ... e il file era stato marcato come "unlinked" (cancellato mentre era aperto)
                if rfs_file.unlinked {
                    should_delete = true;
                    delete_path = rfs_file.file_path.to_string_lossy().to_string();
                }
            }
        }

        // Eseguiamo la cancellazione ritardata (fuori dal borrow di rfs_files)
        if should_delete {
            info!("Chiusura finale file unlinked. Cancellazione fisica: {}", delete_path);
            if let Err(e) = self.cache.api.delete_file_or_directory(&delete_path) {
                error!("Errore cancellazione file ritardata: {}", e);
            }
            // Rimuoviamo definitivamente dalla mappa dei file aperti
            self.rfs_files.remove(&inode);
        }

        reply.ok();
    }
}

fn main() {
    env_logger::builder().filter_level(log::LevelFilter::Info).init();
    let mountpoint = "/mnt/remote-fs";
    let remote_fs = RemoteFS::new();
    std::fs::create_dir_all(mountpoint).unwrap();
    println!("Mounting RemoteFS at {}", mountpoint);
    fuser::mount2(remote_fs, mountpoint, &[MountOption::FSName("remote_fs".to_string()), MountOption::AutoUnmount, MountOption::AllowOther]).unwrap();
}