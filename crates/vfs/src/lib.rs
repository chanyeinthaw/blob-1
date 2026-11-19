use std::collections::HashMap;
use std::ffi::OsStr;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use fuser::{
    BackgroundSession, BsdFileFlags, Config, Errno, FileAttr, FileHandle, FileType, Filesystem,
    FopenFlags, Generation, INodeNo, LockOwner, MountOption, OpenFlags, RenameFlags, ReplyAttr,
    ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyWrite, Request, TimeOrNow,
    WriteFlags,
};
use storage::{Node, NodeId, NodeKind, WorkspaceError, WorkspaceStore};

const TTL: Duration = Duration::from_secs(1);

pub fn mount<S: WorkspaceStore + 'static>(
    store: S,
    mountpoint: impl AsRef<Path>,
) -> io::Result<BackgroundSession> {
    let mountpoint = mountpoint.as_ref();
    let owner = std::fs::metadata(mountpoint)?;
    let mut config = Config::default();
    config.mount_options = vec![
        MountOption::FSName("workspace".into()),
        MountOption::DefaultPermissions,
    ];
    fuser::spawn_mount(
        Workspace {
            store,
            uid: owner.uid(),
            gid: owner.gid(),
            references: Mutex::new(HashMap::new()),
        },
        mountpoint,
        &config,
    )
}

struct Workspace<S> {
    store: S,
    uid: u32,
    gid: u32,
    references: Mutex<HashMap<u64, u64>>,
}

impl<S: WorkspaceStore> Workspace<S> {
    fn attr(&self, node: &Node) -> FileAttr {
        let (kind, nlink) = match (node.kind, node.linked) {
            (NodeKind::File, linked) => (FileType::RegularFile, u32::from(linked)),
            (NodeKind::Directory, linked) => (FileType::Directory, 2 * u32::from(linked)),
        };
        FileAttr {
            ino: INodeNo(node.id.0),
            size: node.size,
            blocks: node.size.div_ceil(512),
            atime: node.modified,
            mtime: node.modified,
            ctime: node.modified,
            crtime: node.modified,
            kind,
            perm: node.mode as u16,
            nlink,
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    fn reference(&self, node: &Node) -> FileAttr {
        *self
            .references
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(node.id.0)
            .or_default() += 1;
        self.attr(node)
    }
}

impl<S: WorkspaceStore + 'static> Filesystem for Workspace<S> {
    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        match utf8(name).and_then(|name| self.store.lookup(id(parent), name).map_err(errno)) {
            Ok(node) => reply.entry(&TTL, &self.reference(&node), Generation(0)),
            Err(error) => reply.error(error),
        }
    }

    fn forget(&self, _req: &Request, ino: INodeNo, nlookup: u64) {
        let released = {
            let mut references = self
                .references
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            match references.get_mut(&ino.0) {
                Some(count) if *count > nlookup => {
                    *count -= nlookup;
                    false
                }
                Some(_) => {
                    references.remove(&ino.0);
                    true
                }
                None => false,
            }
        };
        if released {
            let _ = self.store.forget(id(ino));
        }
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        match self.store.node(id(ino)) {
            Ok(node) => reply.attr(&TTL, &self.attr(&node)),
            Err(error) => reply.error(errno(error)),
        }
    }

    fn setattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<FileHandle>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        let updated = (|| {
            let mut node = self.store.node(id(ino))?;
            if let Some(mode) = mode {
                node = self.store.set_mode(node.id, mode)?;
            }
            if let Some(size) = size {
                node = self.store.truncate(node.id, size)?;
            }
            if let Some(mtime) = mtime {
                let modified = match mtime {
                    TimeOrNow::SpecificTime(time) => time,
                    TimeOrNow::Now => SystemTime::now(),
                };
                node = self.store.set_modified(node.id, modified)?;
            }
            Ok(node)
        })();
        match updated {
            Ok(node) => reply.attr(&TTL, &self.attr(&node)),
            Err(error) => reply.error(errno(error)),
        }
    }

    fn mkdir(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
        let created = utf8(name).and_then(|name| {
            self.store
                .create(id(parent), name, NodeKind::Directory, mode & !umask)
                .map_err(errno)
        });
        match created {
            Ok(node) => reply.entry(&TTL, &self.reference(&node), Generation(0)),
            Err(error) => reply.error(error),
        }
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        empty(
            reply,
            utf8(name).and_then(|name| {
                self.store
                    .remove(id(parent), name, NodeKind::File)
                    .map_err(errno)
            }),
        );
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        empty(
            reply,
            utf8(name).and_then(|name| {
                self.store
                    .remove(id(parent), name, NodeKind::Directory)
                    .map_err(errno)
            }),
        );
    }

    fn rename(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        let replace = if flags.is_empty() {
            true
        } else if flags == RenameFlags::RENAME_NOREPLACE {
            false
        } else {
            return reply.error(Errno::EINVAL);
        };
        let renamed = utf8(name).and_then(|name| {
            let newname = utf8(newname)?;
            self.store
                .rename(id(parent), name, id(newparent), newname, replace)
                .map_err(errno)
        });
        empty(reply, renamed);
    }

    fn read(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        match self.store.read(id(ino), offset, size) {
            Ok(data) => reply.data(&data),
            Err(error) => reply.error(errno(error)),
        }
    }

    fn write(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        match self.store.write(id(ino), offset, data) {
            Ok(_) => reply.written(data.len() as u32),
            Err(error) => reply.error(errno(error)),
        }
    }

    fn flush(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _fh: FileHandle,
        _lock_owner: LockOwner,
        reply: ReplyEmpty,
    ) {
        reply.ok();
    }

    fn fsync(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _fh: FileHandle,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        match self.store.sync() {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(errno(error)),
        }
    }

    fn fsyncdir(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _fh: FileHandle,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        match self.store.sync() {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(errno(error)),
        }
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let children = match self.store.children(id(ino)) {
            Ok(children) => children,
            Err(error) => return reply.error(errno(error)),
        };
        let entries = [
            (ino, FileType::Directory, ".".to_owned()),
            (ino, FileType::Directory, "..".to_owned()),
        ]
        .into_iter()
        .chain(children.into_iter().map(|entry| {
            let kind = match entry.node.kind {
                NodeKind::File => FileType::RegularFile,
                NodeKind::Directory => FileType::Directory,
            };
            (INodeNo(entry.node.id.0), kind, entry.name)
        }));
        for (index, (ino, kind, name)) in entries.enumerate().skip(offset as usize) {
            if reply.add(ino, index as u64 + 1, kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn create(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let created = utf8(name).and_then(|name| {
            self.store
                .create(id(parent), name, NodeKind::File, mode & !umask)
                .map_err(errno)
        });
        match created {
            Ok(node) => reply.created(
                &TTL,
                &self.reference(&node),
                Generation(0),
                FileHandle(0),
                FopenFlags::empty(),
            ),
            Err(error) => reply.error(error),
        }
    }
}

fn id(ino: INodeNo) -> NodeId {
    NodeId(ino.0)
}

fn utf8(name: &OsStr) -> Result<&str, Errno> {
    name.to_str().ok_or(Errno::EINVAL)
}

fn empty(reply: ReplyEmpty, result: Result<(), Errno>) {
    match result {
        Ok(()) => reply.ok(),
        Err(error) => reply.error(error),
    }
}

fn errno(error: WorkspaceError) -> Errno {
    match error {
        WorkspaceError::NotFound => Errno::ENOENT,
        WorkspaceError::AlreadyExists => Errno::EEXIST,
        WorkspaceError::NotADirectory => Errno::ENOTDIR,
        WorkspaceError::IsADirectory => Errno::EISDIR,
        WorkspaceError::NotEmpty => Errno::ENOTEMPTY,
        WorkspaceError::Invalid => Errno::EINVAL,
        WorkspaceError::TooLarge => Errno::EFBIG,
        WorkspaceError::Store(_) => Errno::EIO,
    }
}
