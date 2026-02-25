use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyDirectory, ReplyEntry, ReplyOpen,
    ReplyWrite, ReplyCreate, ReplyEmpty, ReplyData, Request,
};
use libc::{EBADF, ENOENT, EIO, ENOSYS, O_ACCMODE, O_RDWR, O_WRONLY};
use log::{info, error, warn};
use std::{
    collections::{HashMap, HashSet}, ffi::OsStr, time::{Duration, SystemTime}, path::PathBuf,
    sync::atomic::{AtomicU64, Ordering}
};
use shared::file_entry::FileEntry;
use std::cmp;

mod api;
mod cache;
mod file;

use crate::{cache::{Cache, Inode}, file::{OpenFlags, OpenedFile, RfsFile, Fd}};

const TTL: Duration = Duration::from_secs(60);
const PREFETCH_SIZE: u32 = 5 * 1024 * 1024; // 5 MB read ahead

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
            cache: Cache::new(TTL.as_secs()),
            rfs_files: HashMap::new(),
            last_fd: AtomicU64::new(10),
        }
    }

    fn alloc_fd(&self) -> Fd { 
        Fd(self.last_fd.fetch_add(1, Ordering::SeqCst)) 
    }

    fn get_path_str(&self, ino: Inode) -> Option<String> {
        // self.cache.get_file_by_ino(ino).map(|f| f.file_path.to_string_lossy().to_string())
        let cache_lock = self.cache.metadata.read().unwrap();
        cache_lock.get(&ino).map(|m| m.file_path.to_string_lossy().to_string())
    } 

    fn upload_chunk(&mut self, ino: u64, chunk_idx: u64) {
        if let Some(rfs_file) = self.rfs_files.get_mut(&Inode(ino)) {
            if let Some(chunk_data) = rfs_file.read_cache.get(&chunk_idx) {
                let path_str = rfs_file.file_path.to_string_lossy().to_string();
                let chunk_size = PREFETCH_SIZE as u64;
                let offset = chunk_idx * chunk_size;

                // se il file è più piccolo del chunk, non invia tutto ma solo fino alla fine del file
                let end_of_data_in_chunk = if rfs_file.file_entry.size < offset + chunk_size {
                    (rfs_file.file_entry.size - offset) as usize
                } else {
                    chunk_data.len()
                };

                let data_to_send = chunk_data[..end_of_data_in_chunk].to_vec();

                info!("PATCH: Invio {} byte (blocco {}, offset {}) per {}", data_to_send.len(), chunk_idx, offset, path_str);

                match self.cache.api.patch_file_contents(&path_str, offset, data_to_send) {
                    Ok(_) => {
                        rfs_file.dirty_chunks.remove(&chunk_idx);
                        if rfs_file.dirty_chunks.is_empty() {
                            rfs_file.is_dirty = false;
                        }
                    },
                    Err(e) => error!("Errore upload chunk {}: {}", chunk_idx, e),
                }
            }
        }
    }

    fn sync_dirty_chunks(&mut self, ino: u64) {
        let indices: Vec<u64> = if let Some(f) = self.rfs_files.get(&Inode(ino)) {
            f.dirty_chunks.iter().cloned().collect()
        } else {
            return;
        };
        for idx in indices {
            self.upload_chunk(ino, idx);
        }
    }
}

impl Filesystem for RemoteFS {
    
    // --- METADATA ---

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        let mut attr = {
            let cache_lock = self.cache.metadata.read().unwrap();
            match cache_lock.get(&Inode(ino)) {
                Some(f) => FileAttrWrapper::from(f.file_entry.clone()).0,
                None => { reply.error(ENOENT); return; }
            }
        };

        if let Some(rfs_file) = self.rfs_files.get(&Inode(ino)) {
            attr.size = rfs_file.file_entry.size; // size aggiornata
            if rfs_file.is_dirty {
                attr.mtime = SystemTime::now();
            }
        }
        reply.attr(&TTL, &attr);
    }

    fn setattr(
        &mut self, _req: &Request<'_>, ino: u64, _mode: Option<u32>, _uid: Option<u32>, _gid: Option<u32>,
        size: Option<u64>, _atime: Option<fuser::TimeOrNow>, _mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<SystemTime>, _fh: Option<u64>, _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>, _bkuptime: Option<SystemTime>, _flags: Option<u32>, reply: ReplyAttr,
    ) {
        if let Some(new_size) = size {
            if let Some(rfs_file) = self.rfs_files.get_mut(&Inode(ino)) {
                if new_size == 0 {
                    rfs_file.read_cache.clear();
                    rfs_file.dirty_chunks.clear();
                }
                rfs_file.file_entry.size = new_size;
                rfs_file.is_dirty = true;
                // API per troncare il file sul server ?
            }
        }
        self.getattr(_req, ino, None, reply);
    }

    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p, 
            None => { reply.error(ENOENT); return; }
        };
        let target_path = PathBuf::from(&parent_path).join(name.to_string_lossy().as_ref());
        
        let entry = {
            let cache_lock = self.cache.metadata.read().unwrap();
            cache_lock.values().find(|f| f.file_path == target_path).cloned()
        };

        match entry {
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

    // --- OPERAZIONI FILE ---

    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        let inode = Inode(ino);
        let metadata = {
            let cache_lock = self.cache.metadata.read().unwrap();
            cache_lock.get(&inode).cloned()
        };

        if let Some(cached) = metadata {
            let fd = self.alloc_fd();
            let open_flags = OpenFlags::from_flags(flags);
            let rfs_file = self.rfs_files.entry(inode).or_insert_with(|| RfsFile::from(cached));
            rfs_file.fds.insert(fd, OpenedFile { fd, ino: inode, flags: open_flags });
            reply.opened(fd.0, 0);
        } else {
            reply.error(ENOENT);
        }
    }

    fn create(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, _mode: u32, _umask: u32, flags: i32, reply: ReplyCreate) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p, None => { reply.error(ENOENT); return; }
        };
        let new_path = PathBuf::from(&parent_path).join(name);
        let new_path_str = new_path.to_string_lossy().to_string();

        if self.cache.api.write_file(&new_path_str, vec![]).is_err() { 
            reply.error(EIO); 
            return; 
        }

        let _ = self.cache.list_dir(&parent_path);
        let cache_lock = self.cache.metadata.read().unwrap();
        if let Some(cached) = cache_lock.values().find(|f| f.file_path == new_path).cloned() {
            let fd = self.alloc_fd();
            let ino = Inode(cached.file_entry.ino);
            let mut rfs_file = RfsFile::from(cached);
            rfs_file.fds.insert(fd, OpenedFile { fd, ino, flags: OpenFlags::from_flags(flags) });
            self.rfs_files.insert(ino, rfs_file.clone());
            reply.created(&TTL, &FileAttrWrapper::from(rfs_file.file_entry).0, 0, fd.0, 0);
        } else {
            reply.error(EIO);
        }
    }
    /*
    fn read(&mut self, _req: &Request<'_>, ino: u64, _fh: u64, offset: i64, size: u32, _flags: i32, _lock: Option<u64>, reply: ReplyData) {
        let inode = Inode(ino);
        let chunk_size = PREFETCH_SIZE as u64;
        let chunk_idx = offset as u64 / chunk_size;
        let offset_in_chunk = (offset as u64 % chunk_size) as usize;

        let rfs_file = if let Some(f) = self.rfs_files.get_mut(&inode) {
            f
        } else {
            let metadata = self.cache.metadata.read().unwrap().get(&inode).cloned();
            if let Some(m) = metadata {
                self.rfs_files.insert(inode, RfsFile::from(m));
                self.rfs_files.get_mut(&inode).unwrap()
            } else {
                reply.error(ENOENT); return;
            }
        };

        // 1. Hit: Dato presente in RAM (perché letto prima o scritto localmente)
        if let Some(chunk_data) = rfs_file.read_cache.get(&chunk_idx) {
            let end = cmp::min(offset_in_chunk + size as usize, chunk_data.len());
            if offset_in_chunk >= chunk_data.len() {
                reply.data(&[]);
            } else {
                reply.data(&chunk_data[offset_in_chunk..end]);
            }
            return;
        }

        // 2. Miss: Scarica dal server
        let fetch_start = chunk_idx * chunk_size;
        let path_str = rfs_file.file_path.to_string_lossy().to_string();

        info!("Cache Miss: Scarico blocco {} per ino {}", chunk_idx, ino);
        match self.cache.api.read_file_contents(&path_str, fetch_start, chunk_size as u32) {
            Ok(data) => {
                rfs_file.read_cache.insert(chunk_idx, data.clone());
                let end = cmp::min(offset_in_chunk + size as usize, data.len());
                if offset_in_chunk >= data.len() {
                    reply.data(&[]);
                } else {
                    reply.data(&data[offset_in_chunk..end]);
                }
            },
            Err(_) => reply.error(EIO),
        }
    } */

    fn read(&mut self, _req: &Request<'_>, ino: u64, _fh: u64, offset: i64, size: u32, _flags: i32, _lock: Option<u64>, reply: ReplyData) {
        let inode = Inode(ino);
        let chunk_size = PREFETCH_SIZE as u64;
        let chunk_idx = offset as u64 / chunk_size;
        let offset_in_chunk = (offset as u64 % chunk_size) as usize;

        let rfs_file = if let Some(f) = self.rfs_files.get_mut(&inode) {
            f
        } else {
            let metadata = self.cache.metadata.read().unwrap().get(&inode).cloned();
            if let Some(m) = metadata {
                self.rfs_files.insert(inode, RfsFile::from(m));
                self.rfs_files.get_mut(&inode).unwrap()
            } else {
                reply.error(ENOENT); return;
            }
        };

        // controllo EOF
        if offset as u64 >= rfs_file.file_entry.size {
            reply.data(&[]);
            return;
        }

        // cache hit 
        if let Some(chunk_data) = rfs_file.read_cache.get(&chunk_idx) {
            let avail = chunk_data.len();
            let end = cmp::min(offset_in_chunk + size as usize, avail);
            if offset_in_chunk >= avail {
                reply.data(&[]);
            } else {
                reply.data(&chunk_data[offset_in_chunk..end]);
            }
            return;
        }

        // cache miss: scarica, ma solo se il file non è "nuovo" localmente
        let path_str = rfs_file.file_path.to_string_lossy().to_string();
        info!("Cache Miss: Scarico blocco {} per {}", chunk_idx, path_str);

        match self.cache.api.read_file_contents(&path_str, chunk_idx * chunk_size, chunk_size as u32) {
            Ok(data) => {
                rfs_file.read_cache.insert(chunk_idx, data.clone());
                let end = cmp::min(offset_in_chunk + size as usize, data.len());
                reply.data(&data[offset_in_chunk..end]);
            },
            Err(_) => {
                // se il file è vuoto sul server ma noi abbiamo una size > 0 (appena scritto),
                // inizializza un chunk vuoto invece di dare errore.
                if rfs_file.is_dirty {
                    let empty_chunk = vec![0u8; chunk_size as usize];
                    rfs_file.read_cache.insert(chunk_idx, empty_chunk.clone());
                    reply.data(&empty_chunk[offset_in_chunk..cmp::min(offset_in_chunk + size as usize, chunk_size as usize)]);
                } else {
                    reply.error(EIO);
                }
            }
        }
    }
    /*
    fn write(&mut self, _req: &Request<'_>, ino: u64, _fh: u64, offset: i64, data: &[u8], _write_flags: u32, _flags: i32, _lock: Option<u64>, reply: ReplyWrite) {
        let inode = Inode(ino);
        let chunk_size = PREFETCH_SIZE as u64;
        let chunk_idx = offset as u64 / chunk_size;
        let offset_in_chunk = (offset as u64 % chunk_size) as usize;

        let rfs_file = match self.rfs_files.get_mut(&inode) {
            Some(f) => f,
            None => { reply.error(EBADF); return; }
        };

        // Ottieni o inizializza il chunk. 
        // Se non esiste, lo creiamo vuoto (o potresti scaricarlo dal server se è una scrittura parziale)
        let chunk_data = rfs_file.read_cache.entry(chunk_idx).or_insert_with(|| vec![0; chunk_size as usize]);

        let end_in_chunk = cmp::min(offset_in_chunk + data.len(), chunk_size as usize);
        let bytes_to_copy = end_in_chunk - offset_in_chunk;
        chunk_data[offset_in_chunk..end_in_chunk].copy_from_slice(&data[..bytes_to_copy]);

        rfs_file.dirty_chunks.insert(chunk_idx);
        rfs_file.is_dirty = true;

        // Aggiorna size locale
        let write_end = offset as u64 + bytes_to_copy as u64;
        if write_end > rfs_file.file_entry.size {
            rfs_file.file_entry.size = write_end;
        }

        // Streaming: Se abbiamo finito il chunk, caricalo subito
        if end_in_chunk == chunk_size as usize {
            self.upload_chunk(ino, chunk_idx);
        }

        reply.written(bytes_to_copy as u32);
    }*/

    fn write(&mut self, _req: &Request<'_>, ino: u64, _fh: u64, offset: i64, data: &[u8], _write_flags: u32, _flags: i32, _lock: Option<u64>, reply: ReplyWrite) {
        let inode = Inode(ino);
        let chunk_size = PREFETCH_SIZE as u64;
        let chunk_idx = offset as u64 / chunk_size;
        let offset_in_chunk = (offset as u64 % chunk_size) as usize;

        let rfs_file = match self.rfs_files.get_mut(&inode) {
            Some(f) => f,
            None => { reply.error(EBADF); return; }
        };

        // prende il chunk
        let chunk_data = if let Some(existing_chunk) = rfs_file.read_cache.get_mut(&chunk_idx) {
            // il chunk è già in ram
            existing_chunk
        } else {
            // chunk non in ram
            // se l'offset è < la dimensione del file, scarica il vecchio contenuto
            // se è > crea un blocco vuoto
            let mut new_chunk = if (chunk_idx * chunk_size) < rfs_file.file_entry.size {
                let path_str = rfs_file.file_path.to_string_lossy().to_string();
                info!("Write-Fetch: Scarico blocco esistente {} per modifica parziale", chunk_idx);
                self.cache.api.read_file_contents(&path_str, chunk_idx * chunk_size, chunk_size as u32)
                    .unwrap_or_else(|_| vec![0; chunk_size as usize])
            } else {
                vec![0; chunk_size as usize]
            };

            // il chunk è di 5mb?
            if new_chunk.len() < chunk_size as usize {
                new_chunk.resize(chunk_size as usize, 0);
            }

            rfs_file.read_cache.insert(chunk_idx, new_chunk);
            rfs_file.read_cache.get_mut(&chunk_idx).unwrap()
        };

        // scrive nel chunk
        let bytes_to_copy = cmp::min(data.len(), (chunk_size as usize) - offset_in_chunk);
        chunk_data[offset_in_chunk..offset_in_chunk + bytes_to_copy].copy_from_slice(&data[..bytes_to_copy]);

        // rende dirty
        rfs_file.dirty_chunks.insert(chunk_idx);
        rfs_file.is_dirty = true;

        // aggiornamento dim file
        let write_end = offset as u64 + bytes_to_copy as u64;
        if write_end > rfs_file.file_entry.size {
            rfs_file.file_entry.size = write_end;
            info!("Nuova dimensione per ino {}: {} byte", ino, write_end);
        }

        reply.written(bytes_to_copy as u32);
    }

    fn flush(&mut self, _req: &Request<'_>, ino: u64, _fh: u64, _lock_owner: u64, reply: ReplyEmpty) {
        self.sync_dirty_chunks(ino);
        reply.ok();
    }

    fn release(&mut self, _req: &Request<'_>, ino: u64, fh: u64, _flags: i32, _lock: Option<u64>, _flush: bool, reply: ReplyEmpty) {
        let inode = Inode(ino);
        let mut should_delete = false;
        let mut delete_path = String::new();

        if let Some(rfs_file) = self.rfs_files.get_mut(&inode) {
            rfs_file.fds.remove(&Fd(fh));

            if rfs_file.fds.is_empty() {
                // se nessuno lo usa più, libera la RAM
                rfs_file.read_cache.clear();
                rfs_file.dirty_chunks.clear();
                
                if rfs_file.unlinked {
                    should_delete = true;
                    delete_path = rfs_file.file_path.to_string_lossy().to_string();
                }
            }
        }

        if should_delete {
            let _ = self.cache.api.delete_file_or_directory(&delete_path);
            self.rfs_files.remove(&inode);
        }
        reply.ok();
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p, None => { reply.error(ENOENT); return; }
        };
        let target_path = PathBuf::from(&parent_path).join(name);
        let target_path_str = target_path.to_string_lossy().to_string();

        let entry = {
            let cache_lock = self.cache.metadata.read().unwrap();
            cache_lock.values().find(|f| f.file_path == target_path).cloned()
        };

        if let Some(cached) = entry {
            let inode = Inode(cached.file_entry.ino);
            if let Some(rfs_file) = self.rfs_files.get_mut(&inode) {
                if !rfs_file.fds.is_empty() {
                    // file aperto: da rinominare (soft-delete)
                    let new_name = format!(".deleted_{}", inode.0);
                    if self.cache.api.rename(&target_path_str, &new_name).is_ok() {
                        rfs_file.unlinked = true;
                        rfs_file.file_path = PathBuf::from(&parent_path).join(new_name);
                        let _ = self.cache.list_dir(&parent_path);
                        reply.ok();
                        return;
                    }
                }
            }
        }

        match self.cache.api.delete_file_or_directory(&target_path_str) {
            Ok(_) => { let _ = self.cache.list_dir(&parent_path); reply.ok() },
            Err(_) => reply.error(EIO),
        }
    }
}

fn main() {
    env_logger::builder().filter_level(log::LevelFilter::Info).init();
    let mountpoint = "/mnt/remote-fs";
    let remote_fs = RemoteFS::new();
    std::fs::create_dir_all(mountpoint).unwrap();
    info!("Mounting RemoteFS at {}", mountpoint);
    fuser::mount2(remote_fs, mountpoint, &[
        MountOption::FSName("remote_fs".to_string()), 
        MountOption::AutoUnmount, 
        MountOption::AllowOther
    ]).unwrap();
}