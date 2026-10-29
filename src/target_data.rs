//! Private target Data filesystem. It is never mounted over the running OS.
use crate::deployment::Target;
use ext4plus::{DirEntryName, Ext4, Ext4Read, Ext4Write, FileType, FollowSymlinks};
use ext4plus::{
    dir::Dir,
    file::File,
    inode::{InodeCreationOptions, InodeFlags, InodeMode},
};
use mochios_boot_selection::gpt_identity::PartitionRange;
use mochios_device_consent::ConsentRecord;
use std::{io, sync::Arc, time::Duration};
type BoxError = Box<dyn std::error::Error + Send + Sync>;
#[derive(Clone)]
struct Data {
    target: Arc<dyn Target>,
    first: u64,
    bytes: u64,
}
impl Data {
    fn range(&self, offset: u64, len: usize) -> io::Result<()> {
        if offset
            .checked_add(len as u64)
            .is_none_or(|n| n > self.bytes)
        {
            Err(io::Error::other("Data filesystem range overflow"))
        } else {
            Ok(())
        }
    }
}
impl Ext4Read for Data {
    fn read(&self, offset: u64, bytes: &mut [u8]) -> Result<(), BoxError> {
        self.range(offset, bytes.len())?;
        if bytes.is_empty() {
            return Ok(());
        }
        let start = offset / 512 * 512;
        let end = (offset + bytes.len() as u64 + 511) / 512 * 512;
        let mut sector = vec![0; (end - start) as usize];
        self.target.read_at(self.first + start, &mut sector)?;
        bytes.copy_from_slice(
            &sector[(offset - start) as usize..(offset - start) as usize + bytes.len()],
        );
        Ok(())
    }
}
impl Ext4Write for Data {
    fn write(&self, offset: u64, bytes: &[u8]) -> Result<(), BoxError> {
        self.range(offset, bytes.len())?;
        if bytes.is_empty() {
            return Ok(());
        }
        let start = offset / 512 * 512;
        let end = (offset + bytes.len() as u64 + 511) / 512 * 512;
        let mut sectors = vec![0; (end - start) as usize];
        self.target.read_at(self.first + start, &mut sectors)?;
        sectors[(offset - start) as usize..(offset - start) as usize + bytes.len()]
            .copy_from_slice(bytes);
        self.target.write_at(self.first + start, &sectors)?;
        Ok(())
    }
    fn flush(&self) -> Result<(), BoxError> {
        self.target.flush().map_err(Into::into)
    }
}
fn err(e: impl std::fmt::Debug) -> io::Error {
    io::Error::other(format!("Target privacy settings: {e:?}"))
}
fn create(fs: &Ext4, parent_path: &str, name: &str, directory: bool, mode: u16) -> io::Result<()> {
    let parent = fs
        .path_to_inode(
            parent_path.try_into().map_err(err)?,
            FollowSymlinks::ExcludeFinalComponent,
        )
        .map_err(err)?;
    let mut parent = Dir::open_inode(fs, parent).map_err(err)?;
    let kind = if directory {
        FileType::Directory
    } else {
        FileType::Regular
    };
    let type_mode = if directory {
        InodeMode::S_IFDIR
    } else {
        InodeMode::S_IFREG
    };
    let mut inode = fs
        .create_inode(InodeCreationOptions {
            file_type: kind,
            mode: InodeMode::from_bits_truncate(mode) | type_mode,
            uid: 0,
            gid: 0,
            time: Duration::ZERO,
            flags: InodeFlags::empty(),
        })
        .map_err(err)?;
    if directory {
        let index = parent
            .read_dir()
            .map_err(err)?
            .find_map(|e| match e {
                Ok(e) if e.file_name() == "." => Some(Ok(e.inode)),
                Err(e) => Some(Err(err(e))),
                _ => None,
            })
            .ok_or_else(|| err("Missing parent inode"))??;
        let mut child = Dir::init(fs.clone(), inode, index).map_err(err)?;
        parent
            .link(
                DirEntryName::try_from(name).map_err(err)?,
                child.inode_mut(),
            )
            .map_err(err)?;
    } else {
        parent
            .link(DirEntryName::try_from(name).map_err(err)?, &mut inode)
            .map_err(err)?;
    }
    Ok(())
}
pub fn save(
    target: Arc<dyn Target>,
    partition: PartitionRange,
    record: &ConsentRecord,
) -> io::Result<()> {
    if !record.complete() || !record.valid() {
        return Err(err("Incomplete device consent"));
    }
    let bytes = serde_json::to_vec_pretty(record).map_err(err)?;
    if bytes.len() > 64 * 1024 {
        return Err(err("Device consent exceeds its size limit"));
    }
    let storage = Data {
        first: partition
            .first_lba
            .checked_mul(512)
            .ok_or_else(|| err("Partition overflow"))?,
        bytes: partition
            .sectors()
            .checked_mul(512)
            .ok_or_else(|| err("Partition overflow"))?,
        target,
    };
    if storage
        .first
        .checked_add(storage.bytes)
        .is_none_or(|n| n > storage.target.byte_len())
    {
        return Err(err("Partition outside target"));
    }
    let fs = Ext4::load_with_writer(Box::new(storage.clone()), Some(Box::new(storage.clone())))
        .map_err(err)?;
    for (parent, name, mode) in [
        ("/", "var", 0o755),
        ("/var", "config", 0o755),
        ("/var/config", "privacy", 0o700),
    ] {
        let path = format!("{}/{}", parent.trim_end_matches('/'), name);
        match fs.symlink_metadata(path.as_str()) {
            Ok(m) if m.file_type().is_dir() && m.uid() == 0 && m.mode() & 0o022 == 0 => {}
            Ok(m) => {
                return Err(err(format!(
                    "Unsafe privacy directory {path} (uid {}, mode {:o})",
                    m.uid(),
                    m.mode()
                )));
            }
            Err(ext4plus::error::Ext4Error::NotFound) => create(&fs, parent, name, true, mode)?,
            Err(e) => return Err(err(e)),
        }
    }
    let path = "/var/config/privacy/consent.json";
    match fs.symlink_metadata(path) {
        Err(ext4plus::error::Ext4Error::NotFound) => {}
        _ => {
            return Err(err(
                "Installation template already contains a device consent record",
            ));
        }
    }
    create(&fs, "/var/config/privacy", "consent.json", false, 0o600)?;
    let mut file: File = fs.open(path).map_err(err)?;
    let mut done = 0;
    while done < bytes.len() {
        let n = file.write_bytes(&bytes[done..]).map_err(err)?;
        if n == 0 {
            return Err(err("Short consent write"));
        }
        done += n;
    }
    fs.flush().map_err(err)?;
    drop(file);
    drop(fs);
    // A fresh reader, after durable flush, checks persisted bytes and metadata.
    let fs = Ext4::load(Box::new(storage.clone())).map_err(err)?;
    let metadata = fs.symlink_metadata(path).map_err(err)?;
    if !metadata.file_type().is_regular_file()
        || metadata.uid() != 0
        || metadata.gid() != 0
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(err("Incorrect device consent permissions"));
    }
    let actual = fs.read(path).map_err(err)?;
    if actual != bytes || serde_json::from_slice::<ConsentRecord>(&actual).map_err(err)? != *record
    {
        return Err(err("Consent read-back mismatch"));
    }
    storage.target.flush()?;
    Ok(())
}
