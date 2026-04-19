use clap::Parser;
#[cfg(target_os = "linux")]
use daemonize::Daemonize;
use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory,
    ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow,
};
use libc::{EBADF, EIO, ENOENT, ENOTEMPTY};
use log::{error, info};
use shared::file_entry::FileEntry;
use std::{
    cmp,
    collections::HashMap,
    ffi::OsStr,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime},
};
use log:: {debug};
use std::process::Command;
use std::process::exit;

mod api;
mod cache;
mod file;

use crate::{
    cache::{Cache, Inode},
    file::{OpenFlags, OpenedFile, RfsFile},
};

// mezzo MB Read Ahead
const PREFETCH_SIZE: u32 = 512 * 1024; 
// mezzo MB grandezza del blocco di scrittura, per evitare di tenere in RAM file troppo grandi prima di fare flush sul server
const CHUNK_SIZE: usize = 512 * 1024;

// CLI ARGUMENTS 
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Percorso di mount
    #[arg(short, long)]
    mount_point: String,

    /// Disabilita la cache locale
    #[arg(long)]
    no_cache: bool,

    /// Dimensione massima cache LRU 
    #[arg(long, default_value_t = 1000)]
    cache_capacity: usize,

    /// TTL cache in secondi
    #[arg(long, default_value_t = 1)]
    ttl: u64,

    /// daemon mode
    #[arg(long)]
    daemon: bool,

    /// URL del server remoto
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    server_url: String,
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
            kind: if val.is_dir {
                FileType::Directory
            } else {
                FileType::RegularFile
            },
            perm: val.permissions as u16,
            nlink: 1,
            uid: 1000,
            gid: 1000,
            rdev: 0,
            blksize: 512,
            flags: 0,
        })
    }
}

struct RemoteFS {
    cache: Cache,
    /// tiene traccia dei file aperti
    rfs_files: HashMap<Inode, RfsFile>,
    last_fd: AtomicU64,
    // Configurazioni
    ttl: Duration,
}

impl RemoteFS {
    fn new(use_cache: bool, cache_capacity: usize, ttl_secs: u64, server_url: String) -> Self {
        let mut cache = Cache::new(cache_capacity, Duration::from_secs(ttl_secs), server_url.clone(),  use_cache);

        info!("Inizializzazione RemoteFS con cache_capacity={}, ttl={:?}, server_url={}, use_cache={}", cache_capacity, Duration::from_secs(ttl_secs), server_url, use_cache);
        // Inizializziamo manualmente la Root (Inode 1)
        cache.files.put(
            Inode(1),
            crate::cache::CachedFile {
                file_entry: FileEntry {
                    ino: 1,
                    name: "/".to_string(),
                    is_dir: true,
                    size: 0,
                    modified_at: SystemTime::now(),
                    permissions: 0o755,
                },
                file_path: PathBuf::from("/"),
            },
        );

        Self {
            cache,
            rfs_files: HashMap::new(),
            last_fd: AtomicU64::new(10),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    /// alloca un nuovo file descriptor univoco per ogni apertura di file, incrementando un contatore 
    fn alloc_fd(&self) -> Fd {
        Fd(self.last_fd.fetch_add(1, Ordering::SeqCst))
    }

    /// dato l'Inode, ritorna il path completo del file. 
    /// Serve principalmente per tradurre le richieste FUSE (Inode) in operazioni sui path da mandare al server.
    fn get_path_str(&mut self, ino: Inode) -> Option<String> {
        // Gestione speciale Root
        if ino.0 == 1 {
            return Some("/".to_string());
        }
        self.cache
            .get_file_by_ino(ino)
            .map(|f| f.file_path.to_string_lossy().to_string())
    }
}

impl Filesystem for RemoteFS {
    // METADATA

    /// ritorna i metadati specificando l'Inode del file
    fn getattr(&mut self, _req: &Request<'_>, ino: u64, reply: ReplyAttr) {

        // controllo se il file richiesto è tra quelli attualmente aperti
        if let Some(rfs_file) = self.rfs_files.get(&Inode(ino)) {
            let mut attr = FileAttrWrapper::from(rfs_file.file_entry.clone()).0;
            // Se il file è stato modificato localmente(dirty)
            if rfs_file.is_dirty  {
                attr.mtime = SystemTime::now();
            }
            reply.attr(&self.ttl, &attr);
            return;
        }

        // il file richiesto non è tra quelli aperti, cerchiamo nella cache
        match self.cache.get_file_by_ino(Inode(ino)) {
            Some(f) => reply.attr(&self.ttl, &FileAttrWrapper::from(f.file_entry).0),
            None => reply.error(ENOENT),
        }
    }

    fn access(&mut self, _req: &Request<'_>, _ino: u64, _mask: i32, reply: ReplyEmpty) {
        reply.ok();
    }

    /// invocata dal kernel quando vuole modificare i metadati di un file
    /// ci interessa principalmente per gestire i resize dei file
    fn setattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        let inode = Inode(ino);
        if let Some(new_size) = size {
            debug!("FUSE SETATTR: Truncate Inode {} alla nuova dimensione: {} byte", ino, new_size);

            // cerco nei file aperti
            if let Some(rfs_file) = self.rfs_files.get_mut(&inode) {
                // Il troncamento invalida i dati pendenti nel buffer
                rfs_file.write_buffer.clear();
                rfs_file.write_offset = 0;
                
                rfs_file.is_dirty = true;
                rfs_file.file_entry.size = new_size;
            } else {
                debug!("FUSE SETATTR: Fallimento truncate locale per Inode {}", ino);
            }
        } else {
            debug!("FUSE SETATTR: Richiesto truncate su Inode {} ma il file non è nei rfs_files (non aperto)", ino);
        }
        // restituisco i metadati aggiornati 
        self.getattr(_req, ino, reply);
    }


    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let target_path = PathBuf::from(&parent_path).join(name.to_string_lossy().as_ref());

        // Cerchiamo prima nella cache locale
        let mut found = self
            .cache
            .files
            .iter()
            .find(|(_, f)| f.file_path == target_path)
            .map(|(i, f)| (*i, f.clone()));

        // Se non c'è, ALLORA scarichiamo dal server
        if found.is_none() {

            debug!("FUSE LOOKUP: Cache miss per '{}', forzo list_dir sul server", target_path.display());

            let _ = self.cache.list_dir(&parent_path);
            found = self
                .cache
                .files
                .iter()
                .find(|(_, f)| f.file_path == target_path)
                .map(|(i, f)| (*i, f.clone()));
        }

        match found {
            Some((_, f)) => reply.entry(
                &self.ttl,
                &FileAttrWrapper::from(f.file_entry).0,
                0,
            ),
            None => {
                // Su macOS risolve l'errore del comando 'mv'
                #[cfg(target_os = "macos")]
                reply.error(ENOENT);

                // Su Linux manteniamo 
                // Diciamo al Kernel: "Non c'è, ma non ricordartelo! Chiedimelo sempre."  --> per bug di Negative Cache
                #[cfg(target_os = "linux")]
                reply.entry(
                    &Duration::from_secs(0),
                    &FileAttr {
                        ino: 0,
                        size: 0,
                        blocks: 0,
                        atime: SystemTime::UNIX_EPOCH,
                        mtime: SystemTime::UNIX_EPOCH,
                        ctime: SystemTime::UNIX_EPOCH,
                        crtime: SystemTime::UNIX_EPOCH,
                        kind: FileType::RegularFile,
                        perm: 0,
                        nlink: 0,
                        uid: 0,
                        gid: 0,
                        rdev: 0,
                        blksize: 0,
                        flags: 0,
                    },
                    0,
                );
            }
        }
    }

    /// elenca tutte le cartelle ed i file di una directory. 
    /// Viene invocata dal kernel quando un processo fa ls o simili.
    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let path = match self.get_path_str(Inode(ino)) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        match self.cache.list_dir(&path) {
            Ok(entries) => {
                for (i, entry) in entries.iter().enumerate().skip(offset as usize) {
                    let kind = if entry.is_dir {
                        FileType::Directory
                    } else {
                        FileType::RegularFile
                    };
                    if reply.add(entry.ino, (i + 1) as i64, kind, &entry.name) {
                        break;
                    }
                }
                reply.ok();
            }
            Err(e) => {
                error!("FUSE READDIR: Fallimento lettura cartella '{}': {}", path, e);
                reply.error(EIO);
            }
        }
    }

    // --- CREAZIONE / RIMOZIONE ---

    /// crea una nuova directory
    fn mkdir(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        let new_path = PathBuf::from(&parent_path)
            .join(name)
            .to_string_lossy()
            .to_string();

        info!("FUSE MKDIR: Creazione directory '{}'", new_path);

        match self.cache.api.create_directory(&new_path) {
            Ok(_) => {
                debug!("FUSE MKDIR: Successo remoto. Risincronizzazione cache per '{}'", parent_path);

                // forza l'aggiornamento della cartella padre per far vedere la nuova directory
                self.cache.dir_cache.remove(&parent_path);
                let _ = self.cache.list_dir(&parent_path);
                let target_path = PathBuf::from(new_path);

                let found = self
                    .cache
                    .files
                    .iter()
                    .find(|(_, f)| f.file_path == target_path)
                    .map(|(_, f)| f.clone());

                match found {
                    Some(f) => reply.entry(
                        &self.ttl,
                        &FileAttrWrapper::from(f.file_entry).0,
                        0,
                    ),
                    None => {
                        // ERROR: Disallineamento grave tra server e client --> la directory è stata creata sul server ma non riesco a trovarla in cache. 
                        // Forzo invalidazione e rispondo con errore.
                        error!("FUSE MKDIR: Directory creata ma invisibile al server durante il refresh");
                        self.cache.dir_cache.remove(&parent_path);
                        reply.error(EIO)
                    }
                }
            }
            Err(e) => {
                // ERROR: Fallimento della chiamata API
                error!("FUSE MKDIR: Fallimento API per '{}': {}", new_path, e);
                reply.error(EIO)
            } 
        }
    }

    /// crea un nuovo file vuoto, ad esempio touch nuovo_file.txt
    fn mknod(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _rdev: u32,
        reply: ReplyEntry,
    ) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        let new_path = PathBuf::from(&parent_path).join(name);

        info!("FUSE MKNOD: Creazione nuovo file vuoto '{}'", new_path .to_string_lossy().to_string());

        // fa la PUT con body vuoto
        if let Err(e) = self.cache.api.write_file(&new_path.to_string_lossy(), std::io::empty(), 0) {
            error!("FUSE MKNOD: Fallimento API durante la creazione di '{}': {}", new_path.to_string_lossy(), e);
            reply.error(EIO);
            return;
        }

        debug!("FUSE MKNOD: Successo remoto. Sincronizzazione cache per la directory padre");

        // forza l'aggiornamento della cartella padre per far vedere il nuovo file
        self.cache.dir_cache.remove(&parent_path);
        let _ = self.cache.list_dir(&parent_path);
        let found = self
            .cache
            .files
            .iter()
            .find(|(_, f)| f.file_path == new_path)
            .map(|(_, f)| f.clone());

        match found {
            Some(cached) => reply.entry(
                &self.ttl,
                &FileAttrWrapper::from(cached.file_entry).0,
                0,
            ),
            None => {
                error!("FUSE MKNOD: File '{}' creato ma invisibile al server durante il refresh", new_path.to_string_lossy());
                self.cache.dir_cache.remove(&parent_path);
                reply.error(EIO)
            }
        }
    }

    /// crea un file, lo apre e ci scrive dentro i dati passati in fase di creazione (ad esempio echo "Ciao" > nuovo_file.txt)
    fn create(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        let new_path = PathBuf::from(&parent_path).join(name);

        info!("FUSE CREATE: Creazione e apertura simultanea di '{}'", new_path.to_string_lossy().to_string());

        // crea il file vuoto 
        if let Err(e) = self.cache.api.write_file(&new_path.to_string_lossy(), std::io::empty(), 0) {
            error!("FUSE CREATE: Fallimento API per '{}': {}", new_path.to_string_lossy().to_string(), e);
            reply.error(EIO);
            return;
        }

        debug!("FUSE CREATE: Creazione remota completata. Sincronizzazione cache in corso...");

        // aggiorno la cache per far vedere il nuovo file, ricarica la lista per ottenere il nuovo Inode remoto
        self.cache.dir_cache.remove(&parent_path);
        let _ = self.cache.list_dir(&parent_path);
        // cerco il file appena creato nella cache per ottenere i suoi metadati e poterlo aprire subito
        let found = self
            .cache
            .files
            .iter()
            .find(|(_, f)| f.file_path == new_path)
            .map(|(_, f)| f.clone());

        match found {
            // se trovo il file lo apro
            Some(cached) => {
                // creao un nuovo fd per il processo che sta "usando" il file
                let fd = self.alloc_fd();
                let ino = Inode(cached.file_entry.ino);
                let mut rfs_file = RfsFile::from(cached);

                debug!("FUSE CREATE: Assegnato Fd {} all'Inode {}", fd.0, ino.0);

                // inserisco il nuovo fd tra quelli che hanno il file aperto, con flag di scrittura 
                rfs_file.fds.insert(
                    fd,
                    OpenedFile {
                        fd,
                        ino,
                        flags: OpenFlags::WRITE,
                    },
                );

                rfs_file.is_dirty = true;

                let entry_for_reply = rfs_file.file_entry.clone();
                // inserisco il file tra quelli aperti
                self.rfs_files.insert(ino, rfs_file);

                reply.created(
                    &self.ttl,
                    &FileAttrWrapper::from(entry_for_reply).0,
                    0,
                    fd.0,
                    0,
                );
            }
            None => {
                error!("FUSE CREATE: File '{}' creato ma non trovato nel successivo refresh", new_path.to_string_lossy().to_string());
                reply.error(EIO)
            },
        }
    }

    /// gestisce la cancellazione di un file
    /// se elimini un file mentre un programma lo sta ancora leggendo o scrivendo, 
    /// il sistema operativo rimuove il nome dalla cartella, ma non cancella fisicamente i dati dal disco 
    /// finché il programma non chiude il file
    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        let target_path = PathBuf::from(&parent_path).join(name);
        let target_path_str = target_path.to_string_lossy().to_string();

        let found_entry = self
            .cache
            .files
            .iter()
            .find(|(_, f)| f.file_path == target_path)
            .map(|(_, f)| f.clone());

        // controllo se l'Inode si trova nella mappa dei file aperti
        // e se ha almeno un fd "attivo"
        if let Some(cached) = found_entry.clone() {
            let inode = Inode(cached.file_entry.ino);

            let is_open = if let Some(rfs_file) = self.rfs_files.get(&inode) {
                !rfs_file.fds.is_empty()
            } else {
                false
            };

            // se è così non lo elimino subito dal server (verrà eliminato nella release quando si chiuderà l'ultimo fd)
            if is_open {
                // ma lo rinomino, nascondendolo all'utente
                // In Linux, i file che iniziano col punto sono nascosti
                let new_name = format!(".deleted_{}_{}", inode.0, name.to_string_lossy());
                info!(
                    "Unlink su file aperto (ino={}). Rename -> {}",
                    inode.0, new_name
                );

                match self.cache.api.rename(&target_path_str, &new_name) {
                    Ok(_) => {
                        if let Some(rfs_file) = self.rfs_files.get_mut(&inode) {
                            rfs_file.unlinked = true;
                            rfs_file.file_path = PathBuf::from(&parent_path).join(&new_name);
                        }
                        //let _ = self.cache.list_dir(&parent_path);
                        self.cache.invalidate_dir(&parent_path);

                        reply.ok();
                    }
                    Err(e) => {
                        error!("FUSE UNLINK: Errore API rename per soft delete: {}", e);
                        reply.error(EIO)
                    },
                }
                return;
            }
        }

        // il file non è aperto da nessuno, posso eliminarlo subito dal server

        info!("FUSE UNLINK: Eliminazione fisica del file '{}'", target_path_str);
        match self.cache.api.delete_file_or_directory(&target_path_str) {
            Ok(_) => {
                // rimuovi dalla cache locale dei file
                if let Some(cached) = found_entry {
                    debug!("FUSE UNLINK: Rimozione Inode {} dalla cache LRU", cached.file_entry.ino);
                    self.cache.files.pop(&Inode(cached.file_entry.ino));
                }
                // invalido la cache della directory padre
                self.cache.invalidate_dir(&parent_path);

                reply.ok()
            }
            Err(e) => {
                error!("FUSE UNLINK: Fallimento API DELETE per '{}': {}", target_path_str, e);
                reply.error(EIO)
            },
        }
    }

    /// cancella una directory, solo se questa è vuota
    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        let target_path = PathBuf::from(&parent_path).join(name);
        let target = target_path.to_string_lossy().to_string();

        info!("FUSE RMDIR: Eliminazione directory '{}'", target);

        // cancella la directory solo se è vuota (controllo gestito nel backend), altrimenti ritorna ENOTEMPTY
        match self.cache.api.delete_file_or_directory(&target) {
            Ok(_) => {
                // rimuovi la directory eliminata dalla cache locale
                let found = self
                    .cache
                    .files
                    .iter()
                    .find(|(_, f)| f.file_path == target_path)
                    .map(|(i, _)| *i);

                if let Some(ino) = found {
                    debug!("FUSE RMDIR: Rimozione Inode {} (directory) dalla cache LRU", ino.0);
                    self.cache.files.pop(&ino);
                }

                // forza l'aggiornamento della cartella padre
                debug!("FUSE RMDIR: Invalidazione cache per la directory padre '{}'", parent_path);
                self.cache.invalidate_dir(&parent_path);

                reply.ok()
            }
            Err(e) => {
                error!("FUSE RMDIR: Fallimento API DELETE per '{}': {}", target, e);
                reply.error(ENOTEMPTY)
            },
        }
    }

    /// rinomina un file o una directory, eventualmente spostandolo in un'altra cartella
    fn rename(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        new_parent: u64,
        new_name: &OsStr,
        _flags: u32,
        reply: ReplyEmpty,
    ) {
        let parent_path = match self.get_path_str(Inode(parent)) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        let new_parent_path = match self.get_path_str(Inode(new_parent)) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let old_path = PathBuf::from(&parent_path)
            .join(name)
            .to_string_lossy()
            .to_string();


        let new_full_path = PathBuf::from(&new_parent_path).join(new_name);
        let mut new_name_str = new_full_path.to_string_lossy().to_string();
        if !new_name_str.starts_with('/') {
            new_name_str = format!("/{}", new_name_str);
        }

        info!("FUSE RENAME: '{}' -> '{}'", old_path, new_full_path.display());
        // passiamo new_name_str all'API
        match self.cache.api.rename(&old_path, &new_name_str) {
            Ok(_) => {
                // se l'operazione di rename è andata a buon fine, aggiorniamo la cache locale per riflettere il cambiamento
                let target_path = PathBuf::from(&parent_path).join(name);

                // cerchiamo il vecchio file in cache
                let found = self
                    .cache
                    .files
                    .iter()
                    .find(|(_, f)| f.file_path == target_path)
                    .map(|(i, _)| *i);

                if let Some(ino) = found {
                    debug!("FUSE RENAME: Rimozione vecchio percorso dalla cache LRU (Inode {})", ino.0);
                    // rimuoviamo dai metadati cache
                    self.cache.files.pop(&ino);

                    // aggiorniamo il file path anche in rfs_files se è aperto
                    if let Some(rfs_file) = self.rfs_files.get_mut(&ino) {
                        debug!("FUSE RENAME: Aggiornamento percorso file aperto in RAM (Inode {})", ino.0);
                        rfs_file.file_path = new_full_path.clone();
                    }
                }

                // invalidiamo le cartelle per forzare il server a mandare i dati nuovi
                // invalido il padre attuale
                debug!("FUSE RENAME: Invalidazione cache directory sorgente '{}'", parent_path);
                self.cache.invalidate_dir(&parent_path);

                // se il file è stato spostato in un'altra cartella, invalido anche la cartella di destinazione
                if parent_path != new_parent_path {
                    debug!("FUSE RENAME: Invalidazione cache directory destinazione '{}'", new_parent_path);
                    self.cache.invalidate_dir(&new_parent_path);
                }

                reply.ok()
            }
            Err(e) => {
                error!("FUSE RENAME: Fallimento API per rinomina '{}' -> '{}': {}", old_path, new_name_str, e);
                if e.kind() == std::io::ErrorKind::NotFound {
                    reply.error(ENOENT)
                } else {
                    reply.error(EIO)
                }
            },
        }
    }

    // IO

    /// apre un file, restituendo un fd che il kernel userà per le operazioni successive di lettura/scrittura
    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        let inode = Inode(ino);
        // prendo il file cachato corrispondente all'Inode richiesto
        if let Some(cached) = self.cache.get_file_by_ino(inode) {
            let fd = self.alloc_fd();
            let open_flags = OpenFlags::from_flags(flags);

            debug!("FUSE OPEN: Allocato Fd {} per Inode {} (Modalità: {:?})", fd.0, ino, open_flags);
            // cerca l'Inode nella mappa dei file aperti, se non c'è lo inserisce con i metadati presi dalla cache
            let rfs_file = self.rfs_files.entry(inode).or_insert(RfsFile::from(cached));
     
            // inserisco il nuovo fd nella mappa dei fd che hanno il file aperto, con i flag di apertura corretti
            rfs_file.fds.insert(
                fd,
                OpenedFile {
                    fd,
                    ino: inode,
                    flags: open_flags,
                },
            );

            reply.opened(fd.0, 0);
        } else {
            debug!("FUSE OPEN: Fallimento, Inode {} non trovato in cache", ino);
            reply.error(ENOENT);
        }
    }

    /// legge il contenuto di un file aperto
    fn read(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        let inode = Inode(ino);

        if let Some(rfs_file) = self.rfs_files.get_mut(&inode) {
            // se stiamo per leggere ma ci sono dei dati in RAM non ancora salvati,
            // forziamo un upload preventivo al server (auto-flush) per coerenza.
            if rfs_file.is_dirty && !rfs_file.write_buffer.is_empty() {

                let path = rfs_file.file_path.to_string_lossy().to_string();
                let data_to_send = std::mem::take(&mut rfs_file.write_buffer);
                let current_offset = rfs_file.write_offset;

                let cursor = std::io::Cursor::new(data_to_send);
                if let Err(e) = self.cache.api.write_file(&path, cursor, current_offset) {
                    log::error!("FUSE READ: Auto-flush fallito per '{}': {}", path, e);
                }
                // Invalidiamo la cache locale di lettura perché il server ha dati nuovi
                rfs_file.read_buffer.clear();
            }
        }

        // se il file non è stato modificato localmente, o non ha un buffer di scrittura, procedo con la lettura normale
        let rfs_file = match self.rfs_files.get_mut(&inode) {
            Some(f) => f,
            None => {
                // se per qualche motivo il file non è tra quelli aperti, provo a recuperarlo dalla cache 
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

        // se la richiesta di lettura è completamente soddisfabile con i dati già presenti nel buffer di lettura, 
        // restituisco i dati direttamente da lì senza fare ulteriori chiamate al server
        if req_start >= buf_start && req_end <= buf_end {
            let slice_start = (req_start - buf_start) as usize;
            let slice_end = (req_end - buf_start) as usize;
            reply.data(&rfs_file.read_buffer[slice_start..slice_end]);
            return;
        }
        
        // se la richiesta di lettura non è completamente soddisfabile con i dati presenti nel buffer
        // dobbiamo chiedere i dati al server. 
        // Per ottimizzare le prestazioni, invece di chiedere solo i dati strettamente necessari per soddisfare la richiesta,
        // chiediamo un blocco di dati più grande, partendo dall'offset richiesto, in modo da avere già in cache i dati per 
        // eventuali letture future che si sovrappongono a questo intervallo.
        let fetch_size = cmp::max(size, PREFETCH_SIZE);
        let path = rfs_file.file_path.to_str().unwrap().to_string();

        debug!("FUSE READ: Prefetch MISS. Fetch rete per Inode {} (offset: {}, fetch_size: {})", ino, req_start, fetch_size);

        match self
            .cache
            .api
            .read_file_contents(&path, req_start, fetch_size)
        {
            Ok(data) => {
                // scrivo i dati appena letti dal server nel buffer di lettura del file
                rfs_file.read_buffer = data;
                // aggiornando anche l'offset del buffer
                rfs_file.read_buffer_offset = req_start;

                let available = rfs_file.read_buffer.len();
                let slice_len = cmp::min(size as usize, available);
                reply.data(&rfs_file.read_buffer[0..slice_len]);
            }
            Err(e) => {
                error!("FUSE READ: Fallimento fetch rete per Inode {}: {}", ino, e);
                reply.error(EIO)
            }
        }
    }

    /// scrive i dati in un file aperto
    fn write(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _wflags: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyWrite,
    ) {
        // se il file è tra quelli aperti, procedo con la scrittura
        if let Some(rfs_file) = self.rfs_files.get_mut(&Inode(ino)) {
           
            // Se il buffer è vuoto, annotiamo da quale offset stiamo iniziando a scrivere
            if rfs_file.write_buffer.is_empty() {
                rfs_file.write_offset = offset as u64;
            }

            // aggiungiamo i byte che vogliamo scrivere in ram
            rfs_file.write_buffer.extend_from_slice(data);
            rfs_file.is_dirty = true;

            // Aggiorniamo la dimensione del file per FUSE
            let end_offset = offset as u64 + data.len() as u64;
            if end_offset > rfs_file.file_entry.size {
                rfs_file.file_entry.size = end_offset;
            }

            // se il buffer di scrittura ha raggiunto la dimensione del chunk, lo inviamo al server
            if rfs_file.write_buffer.len() >= CHUNK_SIZE {
                let path = rfs_file.file_path.to_string_lossy().to_string();
                
                // std::mem::take sposta i dati dal buffer senza fare cloni pesanti
                let data_to_send = std::mem::take(&mut rfs_file.write_buffer); 
                let current_offset = rfs_file.write_offset;
                
                let cursor = std::io::Cursor::new(data_to_send);
                
                info!("FUSE WRITE: upload al server per '{}'", path);
                if let Err(e) = self.cache.api.write_file(&path, cursor, current_offset) {
                    debug!("FUSE WRITE: Fallimento upload API chunk per '{}': {}", path, e);
                    reply.error(libc::EIO);
                    return;
                }
            }

            reply.written(data.len() as u32);
        } else {
            reply.error(EBADF);
        }
    }

    /// chiamata dal kernel quando un file viene chiuso 
    fn flush(&mut self, _req: &Request<'_>, ino: u64, _fh: u64, _lock: u64, reply: ReplyEmpty) {
        let inode = Inode(ino);

        // vedo se il file è tra quelli aperti
        if let Some(rfs_file) = self.rfs_files.get_mut(&inode) {
            // se il file è stato modificato localmente (dirty) 
            // è necessario fare il flush dei dati sul server per garantire la persistenza delle modifiche e la coerenza dei dati
            if rfs_file.is_dirty {
                let path = rfs_file.file_path.to_string_lossy().to_string();

                // scarichiamo al server eventuali byte rimasti nel buffer 
                if !rfs_file.write_buffer.is_empty() {
                    let data_to_send = std::mem::take(&mut rfs_file.write_buffer);
                    let current_offset = rfs_file.write_offset;
                    
                    let cursor = std::io::Cursor::new(data_to_send);
                    
                    info!("FUSE FLUSH: upload al server per '{}'", path);
                    if let Err(e) = self.cache.api.write_file(&path, cursor, current_offset) {
                        log::error!("FUSE FLUSH: Fallimento upload ultimo pezzo per '{}': {}", path, e);
                        reply.error(libc::EIO);
                        return;
                    }
                }

                // Sincronizzazione metadati post-upload
                debug!("FUSE FLUSH: Upload completato con successo. Sincronizzazione metadati per Inode {}", ino);
                rfs_file.file_entry.modified_at = std::time::SystemTime::now();

                if let Some(mut cached) = self.cache.files.get(&inode).cloned() {
                    cached.file_entry = rfs_file.file_entry.clone();
                    self.cache.files.put(inode, cached);
                }
                
                // resetto il flag
                rfs_file.is_dirty = false;
            
            }
        }
        reply.ok();
    }

    /// si occupa di fare "pulizia" (restituire le risorse al sistema)
    /// e gestisce la cancellazione ritardata dei file che sono stati eliminati mentre erano ancora aperti da qualche processo
    fn release(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        _flags: i32,
        _lock: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        let inode = Inode(ino);
        let mut should_delete = false;
        let mut is_completely_closed = false;
        let mut delete_path = String::new();

        if let Some(rfs_file) = self.rfs_files.get_mut(&inode) {
            // rimuovo il fd dalla lista dei fd che hanno il file aperto
            rfs_file.fds.remove(&Fd(fh));

            // se la mappa è vuota, vuol dire che nessuno ha più questo file aperto, 
            // quindi posso fare la pulizia dei buffer e, se il file era stato eliminato (unlinked), 
            // procedere con la cancellazione fisica dal server
            if rfs_file.fds.is_empty() {
                is_completely_closed = true;
                debug!("FUSE RELEASE: Nessun altro processo usa l'Inode {}. Pulizia risorse locali.", ino);
                // se nel buffer non c'è nulla
                if !rfs_file.is_dirty {
                    // svuota i Vec in RAM
                    rfs_file.write_buffer.clear(); 
                    rfs_file.read_buffer.clear();
                } else {
                    debug!("FUSE RELEASE: L'Inode {} chiuso risulta ancora 'dirty'. Possibile mancato salvataggio!", ino);
                }
                // se il file era stato eliminato mentre era ancora aperto,
                // ora che è stato chiuso da tutti i processi, posso eliminarlo fisicamente dal server
                if rfs_file.unlinked {
                    should_delete = true;
                    delete_path = rfs_file.file_path.to_string_lossy().to_string();
                    debug!("FUSE RELEASE: Trovato flag 'unlinked' per Inode {}. Preparazione Hard Delete.", ino);
                }
            }
        }

        // elimini definitivamente il file chiamando l'API
        if should_delete {
            info!("FUSE RELEASE: Final delete (delayed) per: {}", delete_path);

            if let Err(e) = self.cache.api.delete_file_or_directory(&delete_path) {
                error!("FUSE RELEASE: Errore critico. Impossibile eliminare dal server '{}': {}", delete_path, e);
            }
        } 
        // se il file è completamente chiuso, rimuovo l'Inode dalla mappa dei file aperti per liberare memoria
        if is_completely_closed {
            self.rfs_files.remove(&inode);
        }

        reply.ok();
    }
}

fn main() {
    //env_logger::builder().filter_level(log::LevelFilter::Info).init();
    /*env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .init();*/
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();


    let args = Args::parse();
    let mountpoint = args.mount_point.clone();

    // gestione graceful shutdown 
    let mp_handler = mountpoint.clone();


    ctrlc::set_handler(move || {
        info!("Segnale di interruzione ricevuto. Smontaggio in corso...");
            
        // Logica di smontaggio per LINUX
        #[cfg(target_os = "linux")]
        let status = std::process::Command::new("fusermount3")
            .arg("-u")
            .arg(&mp_handler)
            .status()
            .unwrap_or_else(|_| {
                std::process::Command::new("umount")
                    .arg(&mp_handler)
                    .status()
                    .expect("Comandi di smontaggio non trovati")
            });

        // Logica di smontaggio per MACOS
        #[cfg(target_os = "macos")]
        let status = std::process::Command::new("diskutil")
            .arg("unmount")
            .arg("force")
            .arg(&mp_handler)
            .status()
            .unwrap_or_else(|_| {
                std::process::Command::new("umount")
                    .arg(&mp_handler)
                    .status()
                    .expect("Comandi di smontaggio non trovati")
            });


        if status.success() {
            info!("Comando di smontaggio inviato al kernel. Il loop FUSE si chiuderà a breve.");
        } else {
            error!("Smontaggio FALLITO (Device Busy). Assicurati che nessun terminale o programma stia usando il mount point.");
        }
    }).expect("Errore nell'impostazione del gestore segnali");


    if args.daemon {
        #[cfg(target_os = "linux")]
        {
            // salvo i log
            let log_file = std::fs::File::create("/tmp/rfs.log").unwrap();
            match Daemonize::new()
                .pid_file("/tmp/rfs.pid")
                .stdout(log_file.try_clone().unwrap())
                .stderr(log_file)
                .start()
            {
                Ok(_) => info!("Daemon avviato."),
                Err(e) => {
                    eprintln!("Errore daemonize: {}", e);
                    return;
                }
            }
        }

        #[cfg(target_os = "macos")]
        if args.daemon {
            log::warn!("La modalità daemon non è supportata su macOS. Esecuzione in foreground.");
        
        } 
    }

    // inizializzo il file system, passando i parametri di configurazione per la cache
    let fs = RemoteFS::new(!args.no_cache, args.cache_capacity, args.ttl, args.server_url.clone());

    // creo la cartella di mount se non esiste già
    if !std::path::Path::new(&mountpoint).exists() {
        std::fs::create_dir_all(&mountpoint).unwrap();
    }

    info!(
        "Mounting at {} (Cache: {}, TTL: {}s)",
        mountpoint, !args.no_cache, args.ttl
    );

    // Opzioni di base per il mount
    #[allow(unused_mut)]
    let mut options = vec![
        MountOption::FSName("remote_fs".to_string()),
        MountOption::AutoUnmount,
    ];
    
    #[cfg(target_os = "linux")]
    options.push(MountOption::AllowOther);

    if let Err(e) = fuser::mount2(
        fs,
        &mountpoint,
        &options,
    ) {
        error!("Errore mount: {}", e);
    }
    info!("File system smontato in modo pulito. Terminazione del demone.");
}
