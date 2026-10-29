//! Installation mechanics owned by Installer, independent of UI and activation.
use crate::{backend::Progress, image_manifest};
use mochios_boot_selection::{
    BootRecord,
    gpt_identity::{self, SectorReader, UpdateLayout},
};
use mochios_system_image::{
    Architecture, ArtifactDigests, MANIFEST_LEN, Manifest as SlotManifest, SLOT_HEADER_LEN,
    SlotHeader,
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
    sync::Arc,
};

pub trait Target: Send + Sync {
    fn byte_len(&self) -> u64;
    fn read_at(&self, offset: u64, bytes: &mut [u8]) -> io::Result<()>;
    fn write_at(&self, offset: u64, bytes: &[u8]) -> io::Result<()>;
    fn flush(&self) -> io::Result<()>;
}
impl Target for appcore::storage::StorageRegion {
    fn byte_len(&self) -> u64 {
        self.byte_len()
    }
    fn read_at(&self, o: u64, b: &mut [u8]) -> io::Result<()> {
        self.read_at(o, b)
            .map_err(|e| io::Error::other(format!("Storage read failed: {e:?}")))
    }
    fn write_at(&self, o: u64, b: &[u8]) -> io::Result<()> {
        self.write_at(o, b)
            .map_err(|e| io::Error::other(format!("Storage write failed: {e:?}")))
    }
    fn flush(&self) -> io::Result<()> {
        self.flush()
            .map_err(|e| io::Error::other(format!("Storage flush failed: {e:?}")))
    }
}
fn error(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}
fn fail<E: std::fmt::Debug>(e: E) -> io::Error {
    error(format!("Installation verification failed: {e:?}"))
}
fn number(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn word(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn put(b: &mut [u8], o: usize, n: u64) {
    b[o..o + 8].copy_from_slice(&n.to_le_bytes());
}
fn crc(b: &[u8]) -> u32 {
    let mut c = !0u32;
    for byte in b {
        c ^= *byte as u32;
        for _ in 0..8 {
            c = (c >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(c & 1)));
        }
    }
    !c
}
fn fix_header(b: &mut [u8]) {
    b[16..20].fill(0);
    let c = crc(&b[..92]);
    b[16..20].copy_from_slice(&c.to_le_bytes());
}
fn canonical_guid(b: &[u8]) -> [u8; 16] {
    let mut g: [u8; 16] = b.try_into().unwrap();
    g[..4].reverse();
    g[4..6].reverse();
    g[6..8].reverse();
    g
}

pub struct Source {
    file: File,
    pub manifest: image_manifest::Manifest,
    pub gpt: Gpt,
    keys: Vec<[u8; 32]>,
}
struct FileSectors<'a>(&'a mut File);
impl SectorReader for FileSectors<'_> {
    type Error = io::Error;
    fn read_sector(&mut self, lba: u64, b: &mut [u8; 512]) -> io::Result<()> {
        self.0.seek(SeekFrom::Start(
            lba.checked_mul(512)
                .ok_or_else(|| error("Sector overflow"))?,
        ))?;
        self.0.read_exact(b)
    }
}
#[derive(Clone)]
pub struct Gpt {
    primary: [u8; 512],
    backup: [u8; 512],
    table: Vec<u8>,
    pmbr: [u8; 512],
    pub layout: UpdateLayout,
    pub sectors: u64,
}
impl Gpt {
    fn load(file: &mut File, sectors: u64) -> io::Result<Self> {
        let mut reader = FileSectors(file);
        let mut primary = [0; 512];
        let mut backup = [0; 512];
        let mut pmbr = [0; 512];
        reader.read_sector(0, &mut pmbr)?;
        reader.read_sector(1, &mut primary)?;
        reader.read_sector(sectors - 1, &mut backup)?;
        if pmbr[510..] != [0x55, 0xaa] || pmbr[450] != 0xee || word(&pmbr, 454) != 1 {
            return Err(error("Missing protective MBR"));
        }
        for (header, at, other, table_at) in [
            (&primary, 1, sectors - 1, 2),
            (&backup, sectors - 1, 1, sectors - 33),
        ] {
            if &header[..8] != b"EFI PART"
                || word(header, 8) != 0x10000
                || word(header, 12) != 92
                || word(header, 20) != 0
                || number(header, 24) != at
                || number(header, 32) != other
                || number(header, 72) != table_at
                || word(header, 80) != 128
                || word(header, 84) != 128
                || header[92..].iter().any(|b| *b != 0)
            {
                return Err(error("Unsupported installation GPT"));
            }
            let mut checked = *header;
            checked[16..20].fill(0);
            if crc(&checked[..92]) != word(header, 16) {
                return Err(error("GPT header checksum mismatch"));
            }
        }
        if primary[40..72] != backup[40..72] || primary[80..92] != backup[80..92] {
            return Err(error("GPT copies disagree"));
        }
        let mut table = vec![0; 16384];
        reader.0.seek(SeekFrom::Start(1024))?;
        reader.0.read_exact(&mut table)?;
        let mut second = vec![0; 16384];
        reader.0.seek(SeekFrom::Start((sectors - 33) * 512))?;
        reader.0.read_exact(&mut second)?;
        if table != second || crc(&table) != word(&primary, 88) {
            return Err(error("GPT partition checksum mismatch"));
        }
        let esp_type = [
            0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0, 0xa0, 0xc9, 0x3e, 0xc9,
            0x3b,
        ];
        let esps: Vec<_> = table
            .chunks_exact(128)
            .filter(|e| e[..16] == esp_type)
            .collect();
        if esps.len() != 1 {
            return Err(error("The installation template must contain one ESP"));
        }
        let layout = gpt_identity::find_update_layout(
            &mut reader,
            sectors,
            canonical_guid(&esps[0][16..32]),
        )
        .map_err(fail)?
        .ok_or_else(|| error("Missing A/B installation layout"))?;
        if layout.esp.first_lba < 34
            || layout.state.sectors() < 16
            || number(&primary, 48) > sectors - 34
        {
            return Err(error("Invalid installation ranges"));
        }
        let used: Vec<_> = table
            .chunks_exact(128)
            .filter(|e| e[..16] != [0; 16])
            .collect();
        if used.len() != 7 {
            return Err(error("Unexpected installation partitions"));
        }
        let mut ids = std::collections::BTreeSet::new();
        for e in used {
            if !ids.insert(e[16..32].to_vec()) {
                return Err(error("Duplicate partition identity"));
            }
        }
        Ok(Self {
            primary,
            backup,
            table,
            pmbr,
            layout,
            sectors,
        })
    }
    pub fn for_target(
        &self,
        bytes: u64,
        mut random: impl FnMut(&mut [u8]) -> io::Result<()>,
    ) -> io::Result<Self> {
        if bytes % 512 != 0 || bytes / 512 < self.sectors {
            return Err(error("The installation destination is too small"));
        }
        let mut result = self.clone();
        result.sectors = bytes / 512;
        let mut used = std::collections::BTreeSet::new();
        let mut guid = |b: &mut [u8]| -> io::Result<()> {
            random(b)?;
            if b.iter().all(|b| *b == 0) || !used.insert(b.to_vec()) {
                return Err(error("Random device identity generation failed"));
            }
            Ok(())
        };
        guid(&mut result.primary[56..72])?;
        for entry in result
            .table
            .chunks_exact_mut(128)
            .filter(|e| e[..16] != [0; 16])
        {
            guid(&mut entry[16..32])?;
        }
        put(&mut result.primary, 32, result.sectors - 1);
        put(&mut result.primary, 48, result.sectors - 34);
        result.primary[88..92].copy_from_slice(&crc(&result.table).to_le_bytes());
        fix_header(&mut result.primary);
        result.backup = result.primary;
        put(&mut result.backup, 24, result.sectors - 1);
        put(&mut result.backup, 32, 1);
        put(&mut result.backup, 72, result.sectors - 33);
        fix_header(&mut result.backup);
        result.pmbr[458..462]
            .copy_from_slice(&((result.sectors - 1).min(u32::MAX as u64) as u32).to_le_bytes());
        Ok(result)
    }
    // Bind parsed metadata to the very bytes covered by the full-image hash.
    // A file descriptor pins an inode, but does not make its contents immutable.
    fn check_chunk(&self, offset: u64, bytes: &[u8]) -> io::Result<()> {
        for (start, expected) in [
            (0, self.pmbr.as_slice()),
            (512, self.primary.as_slice()),
            (1024, self.table.as_slice()),
            ((self.sectors - 33) * 512, self.table.as_slice()),
            ((self.sectors - 1) * 512, self.backup.as_slice()),
        ] {
            let first = offset.max(start);
            let last = (offset + bytes.len() as u64).min(start + expected.len() as u64);
            if first < last
                && bytes[(first - offset) as usize..(last - offset) as usize]
                    != expected[(first - start) as usize..(last - start) as usize]
            {
                return Err(error(
                    "Installation partition metadata changed during verification",
                ));
            }
        }
        Ok(())
    }
    fn publish(&self, target: &dyn Target) -> io::Result<()> {
        for (offset, bytes) in [
            ((self.sectors - 33) * 512, self.table.as_slice()),
            ((self.sectors - 1) * 512, self.backup.as_slice()),
            (1024, self.table.as_slice()),
            (512, self.primary.as_slice()),
            (0, self.pmbr.as_slice()),
        ] {
            target.write_at(offset, bytes)?;
            target.flush()?;
            let mut actual = vec![0; bytes.len()];
            target.read_at(offset, &mut actual)?;
            if actual != bytes {
                return Err(error("Partition metadata read-back mismatch"));
            }
        }
        Ok(())
    }
}
impl Source {
    pub fn open(
        directory: &Path,
        version: &str,
        build: u64,
        keys: &[[u8; 32]],
    ) -> io::Result<Self> {
        let mut manifest = File::open(directory.join("disk.manifest"))?;
        if !manifest.metadata()?.is_file()
            || manifest.metadata()?.len() != image_manifest::LENGTH as u64
        {
            return Err(error("Invalid installation manifest file"));
        }
        let mut signed = [0; image_manifest::LENGTH];
        manifest.read_exact(&mut signed)?;
        let manifest = image_manifest::Manifest::verify(&signed, keys).map_err(error)?;
        if manifest.version != version || manifest.build != build {
            return Err(error(
                "Installation source does not match this Installer release",
            ));
        }
        let mut file = File::open(directory.join("disk.img"))?;
        if !file.metadata()?.is_file() || file.metadata()?.len() != manifest.bytes {
            return Err(error("Invalid installation image file"));
        }
        let gpt = Gpt::load(&mut file, manifest.bytes / 512)?;
        file.seek(SeekFrom::Start(0))?;
        let mut digest = Sha256::new();
        let mut buffer = vec![0; 1024 * 1024];
        let mut left = manifest.bytes;
        while left != 0 {
            let n = left.min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..n])?;
            gpt.check_chunk(manifest.bytes - left, &buffer[..n])?;
            digest.update(&buffer[..n]);
            left -= n as u64;
        }
        if digest.finalize().as_slice() != manifest.sha256 {
            return Err(error("Installation image checksum mismatch"));
        }
        let mut result = Self {
            file,
            manifest,
            gpt,
            keys: keys.to_vec(),
        };
        result.verify_boot()?;
        Ok(result)
    }
    fn bytes(&mut self, offset: u64, length: usize) -> io::Result<Vec<u8>> {
        if offset
            .checked_add(length as u64)
            .is_none_or(|n| n > self.manifest.bytes)
        {
            return Err(error("Source range overflow"));
        }
        let mut b = vec![0; length];
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(&mut b)?;
        Ok(b)
    }
    fn hash(&mut self, offset: u64, length: u64) -> io::Result<[u8; 32]> {
        let mut hash = Sha256::new();
        let mut done = 0;
        while done < length {
            let n = (length - done).min(1024 * 1024) as usize;
            hash.update(self.bytes(offset + done, n)?);
            done += n as u64;
        }
        Ok(hash.finalize().into())
    }
    fn verify_boot(&mut self) -> io::Result<()> {
        let layout = self.gpt.layout;
        for i in 0..2 {
            let boot = layout.boot[i];
            let system = layout.system[i];
            let start = boot.first_lba * 512;
            let header = SlotHeader::decode(&self.bytes(start, SLOT_HEADER_LEN)?).map_err(fail)?;
            if header.image_size != boot.sectors() * 512
                || header.architecture != Architecture::X86_64
            {
                return Err(error("Invalid boot slot"));
            }
            let manifest =
                SlotManifest::decode(&self.bytes(start + header.manifest.offset, MANIFEST_LEN)?)
                    .map_err(fail)?;
            if manifest.version() != self.manifest.version
                || manifest.build() != self.manifest.build
                || manifest.image_size() != system.sectors() * 512
                || manifest.architecture() != Architecture::X86_64
            {
                return Err(error("Slot release mismatch"));
            }
            let digests = ArtifactDigests {
                system: self.hash(system.first_lba * 512, system.sectors() * 512)?,
                kernel: self.hash(start + header.kernel.offset, header.kernel.length)?,
                kernel_meta: self
                    .hash(start + header.kernel_meta.offset, header.kernel_meta.length)?,
                initfs: self.hash(start + header.initfs.offset, header.initfs.length)?,
            };
            manifest.verify(&digests, &self.keys).map_err(fail)?;
        }
        let state = layout.state.first_lba * 512;
        for offset in [0, 4096] {
            if self.bytes(state + offset, 32)? != BootRecord::initial().encode() {
                return Err(error(
                    "The installation template is not in initial boot state",
                ));
            }
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Prepared,
    Copied,
    ConsentSaved,
    Complete,
    Failed,
}
pub struct Deployment {
    source: Source,
    target: Arc<dyn Target>,
    gpt: Gpt,
    stage: Stage,
}
impl Deployment {
    pub fn new(
        source: Source,
        target: Arc<dyn Target>,
        random: impl FnMut(&mut [u8]) -> io::Result<()>,
    ) -> io::Result<Self> {
        let gpt = source.gpt.for_target(target.byte_len(), random)?;
        Ok(Self {
            source,
            target,
            gpt,
            stage: Stage::Prepared,
        })
    }
    pub fn copy(&mut self, progress: &mut dyn FnMut(Progress)) -> io::Result<()> {
        if self.stage != Stage::Prepared {
            return Err(error("Invalid installation stage"));
        }
        self.stage = Stage::Failed;
        progress(Progress::stage("Preparing the installation destination…"));
        self.target.write_at(0, &vec![0; 34 * 512])?;
        self.target
            .write_at((self.gpt.sectors - 33) * 512, &vec![0; 33 * 512])?;
        self.target.flush()?;
        let total = self.source.manifest.bytes;
        let mut offset = 0;
        let mut hash = Sha256::new();
        while offset < total {
            let n = (total - offset).min(1024 * 1024) as usize;
            let bytes = self.source.bytes(offset, n)?;
            self.source.gpt.check_chunk(offset, &bytes)?;
            hash.update(&bytes);
            let start = offset.max(34 * 512);
            let end = (offset + n as u64).min(total - 33 * 512);
            if start < end {
                let body = &bytes[(start - offset) as usize..(end - offset) as usize];
                self.target.write_at(start, body)?;
            }
            offset += n as u64;
            progress(Progress {
                stage: "Installing mochiOS…".into(),
                completed: Some(offset),
                total: Some(total),
            });
        }
        if hash.finalize().as_slice() != self.source.manifest.sha256 {
            return Err(error("Installation source changed during copying"));
        }
        self.target.flush()?;
        progress(Progress::stage("Verifying installed data…"));
        self.compare(34 * 512, total - 33 * 512 - 34 * 512)?;
        self.stage = Stage::Copied;
        Ok(())
    }
    fn compare(&mut self, offset: u64, length: u64) -> io::Result<()> {
        let mut done = 0;
        while done < length {
            let n = (length - done).min(1024 * 1024) as usize;
            let expected = self.source.bytes(offset + done, n)?;
            let mut actual = vec![0; n];
            self.target.read_at(offset + done, &mut actual)?;
            if actual != expected {
                return Err(error("Installed data read-back mismatch"));
            }
            done += n as u64;
        }
        Ok(())
    }
    pub fn save_consent(
        &mut self,
        record: &mochios_device_consent::ConsentRecord,
    ) -> io::Result<()> {
        if self.stage != Stage::Copied {
            return Err(error("Invalid consent persistence stage"));
        }
        self.stage = Stage::Failed;
        crate::target_data::save(self.target.clone(), self.gpt.layout.data, record)?;
        self.stage = Stage::ConsentSaved;
        Ok(())
    }
    pub fn finish(&mut self, progress: &mut dyn FnMut(Progress)) -> io::Result<()> {
        if self.stage != Stage::ConsentSaved {
            return Err(error("Installation settings have not been verified"));
        }
        self.stage = Stage::Failed;
        progress(Progress::stage(
            "Verifying boot files and finalizing installation…",
        ));
        let l = self.gpt.layout;
        for part in [
            l.esp,
            l.boot[0],
            l.system[0],
            l.boot[1],
            l.system[1],
            l.state,
        ] {
            self.compare(part.first_lba * 512, part.sectors() * 512)?;
        }
        self.target.flush()?;
        self.gpt.publish(self.target.as_ref())?;
        self.target.flush()?;
        self.stage = Stage::Complete;
        Ok(())
    }
}

#[cfg(test)]
#[path = "deployment_tests.rs"]
mod tests;
