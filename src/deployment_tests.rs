//! Real GPT, signed boot slots and ext4 writes, all on temporary regular files.
use super::*;
use ed25519_dalek::SigningKey;
use mochios_device_consent::{ConsentRecord, DataCategory, DocumentReceipt};
use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "mochios-install-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Fixture {
    root: Temp,
    key: SigningKey,
    bytes: u64,
}
fn run(command: &mut Command) {
    let result = command
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .expect("Required host image tool could not be executed");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn fixture() -> Arc<Fixture> {
    static FIXTURE: OnceLock<Mutex<std::sync::Weak<Fixture>>> = OnceLock::new();
    let mut cache = FIXTURE
        .get_or_init(|| Mutex::new(std::sync::Weak::new()))
        .lock()
        .unwrap();
    if let Some(fixture) = cache.upgrade() {
        return fixture;
    }
    let fixture = Arc::new({
        let root = Temp::new();
        let key = SigningKey::from_bytes(&[9; 32]);
        let stage = root.0.join("seed");
        fs::create_dir_all(stage.join("var/config")).unwrap();
        let data = root.0.join("data.img");
        File::create(&data)
            .unwrap()
            .set_len(16 * 1024 * 1024)
            .unwrap();
        run(Command::new("fakeroot").args(["sh","-c",r#"chown -R 0:0 -- "$1"; chmod -R go-w -- "$1"; exec mke2fs -q -t ext4 -O ^has_journal -b 4096 -d "$1" -F "$2""#,"fixture"]).arg(&stage).arg(&data));
        let system = vec![0u8; 8 * 1024 * 1024];
        let kernel = b"test kernel";
        let meta = b"test metadata";
        let initfs = b"test initfs";
        let slot_manifest = SlotManifest::create(
            "26.0.0",
            42,
            Architecture::X86_64,
            system.len() as u64,
            ArtifactDigests {
                system: hash(&system),
                kernel: hash(kernel),
                kernel_meta: hash(meta),
                initfs: hash(initfs),
            },
            &key,
        )
        .unwrap();
        let (header, layout) = SlotHeader::create(
            Architecture::X86_64,
            1024 * 1024,
            kernel.len() as u64,
            meta.len() as u64,
            initfs.len() as u64,
        )
        .unwrap();
        let mut boot = vec![0u8; 1024 * 1024];
        boot[..SLOT_HEADER_LEN].copy_from_slice(&header);
        for (region, bytes) in [
            (layout.manifest, slot_manifest.as_bytes().as_slice()),
            (layout.kernel, kernel.as_slice()),
            (layout.kernel_meta, meta.as_slice()),
            (layout.initfs, initfs.as_slice()),
        ] {
            boot[region.offset as usize..region.offset as usize + bytes.len()]
                .copy_from_slice(bytes);
        }
        let bytes = 104 * 1024 * 1024;
        let image = root.0.join("disk.img");
        let mut file = File::create(&image).unwrap();
        file.set_len(bytes).unwrap();
        let mut process = Command::new("sfdisk")
            .arg(&image)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let entries = [
            (2048, 131072, "U", "mochiOS ESP"),
            (
                133120,
                2048,
                "6d6f6368-694f-5300-8000-6d5061727404",
                "mochiOS Boot A",
            ),
            (
                135168,
                16384,
                "6d6f6368-694f-5300-8000-6d5061727401",
                "mochiOS System A",
            ),
            (
                151552,
                2048,
                "6d6f6368-694f-5300-8000-6d5061727404",
                "mochiOS Boot B",
            ),
            (
                153600,
                16384,
                "6d6f6368-694f-5300-8000-6d5061727401",
                "mochiOS System B",
            ),
            (
                169984,
                32768,
                "6d6f6368-694f-5300-8000-6d5061727402",
                "mochiOS Data",
            ),
            (
                202752,
                2048,
                "6d6f6368-694f-5300-8000-6d5061727403",
                "mochiOS Boot State",
            ),
        ];
        {
            let input = process.stdin.as_mut().unwrap();
            writeln!(
                input,
                "label: gpt\nunit: sectors\nfirst-lba: 2048\nsector-size: 512\n"
            )
            .unwrap();
            for (start, size, kind, name) in entries {
                writeln!(
                    input,
                    "start={start}, size={size}, type={kind}, name=\"{name}\""
                )
                .unwrap();
            }
        }
        drop(process.stdin.take());
        let result = process.wait_with_output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        for (lba, bytes) in [
            (133120, boot.as_slice()),
            (135168, system.as_slice()),
            (151552, boot.as_slice()),
            (153600, system.as_slice()),
        ] {
            file.seek(SeekFrom::Start(lba * 512)).unwrap();
            file.write_all(bytes).unwrap();
        }
        file.seek(SeekFrom::Start(169984 * 512)).unwrap();
        file.write_all(&fs::read(data).unwrap()).unwrap();
        for offset in [0, 4096] {
            file.seek(SeekFrom::Start(202752 * 512 + offset)).unwrap();
            file.write_all(&BootRecord::initial().encode()).unwrap();
        }
        file.sync_all().unwrap();
        drop(file);
        let manifest = image_manifest::Manifest {
            version: "26.0.0".into(),
            build: 42,
            bytes,
            sha256: hash(&fs::read(image).unwrap()),
            architecture: 1,
        };
        fs::write(root.0.join("disk.manifest"), manifest.sign(&key).unwrap()).unwrap();
        Fixture { root, key, bytes }
    });
    *cache = Arc::downgrade(&fixture);
    fixture
}
fn source() -> Source {
    let f = fixture();
    Source::open(&f.root.0, "26.0.0", 42, &[f.key.verifying_key().to_bytes()]).unwrap()
}
struct Disk {
    file: Mutex<File>,
    bytes: u64,
    writes: AtomicU64,
    flushes: AtomicU64,
    fail_flush: AtomicU64,
    fail_write: AtomicU64,
    corrupt_reads: std::sync::atomic::AtomicBool,
}
impl Disk {
    fn new(root: &Temp, bytes: u64) -> Arc<Self> {
        let file = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(root.0.join("target.img"))
            .unwrap();
        file.set_len(bytes).unwrap();
        Arc::new(Self {
            file: Mutex::new(file),
            bytes,
            writes: AtomicU64::new(0),
            flushes: AtomicU64::new(0),
            fail_flush: AtomicU64::new(u64::MAX),
            fail_write: AtomicU64::new(u64::MAX),
            corrupt_reads: std::sync::atomic::AtomicBool::new(false),
        })
    }
}
impl Target for Disk {
    fn byte_len(&self) -> u64 {
        self.bytes
    }
    fn read_at(&self, o: u64, b: &mut [u8]) -> io::Result<()> {
        if o.checked_add(b.len() as u64).is_none_or(|n| n > self.bytes) {
            return Err(error("Test target overflow"));
        }
        let mut f = self.file.lock().unwrap();
        f.seek(SeekFrom::Start(o))?;
        f.read_exact(b)?;
        if o >= 34 * 512 && !b.is_empty() && self.corrupt_reads.load(Ordering::Relaxed) {
            b[0] ^= 1;
        }
        Ok(())
    }
    fn write_at(&self, o: u64, b: &[u8]) -> io::Result<()> {
        if o.checked_add(b.len() as u64).is_none_or(|n| n > self.bytes) {
            return Err(error("Test target overflow"));
        }
        let count = self.writes.fetch_add(1, Ordering::Relaxed) + 1;
        if count == self.fail_write.load(Ordering::Relaxed) {
            return Err(error("Injected write failure"));
        }
        let mut f = self.file.lock().unwrap();
        f.seek(SeekFrom::Start(o))?;
        f.write_all(b)
    }
    fn flush(&self) -> io::Result<()> {
        let n = self.flushes.fetch_add(1, Ordering::Relaxed) + 1;
        if n == self.fail_flush.load(Ordering::Relaxed) {
            return Err(error("Injected flush failure"));
        }
        self.file.lock().unwrap().sync_all()
    }
}
fn rng() -> impl FnMut(&mut [u8]) -> io::Result<()> {
    let mut n = 0u8;
    move |b| {
        n += 1;
        b.fill(n);
        Ok(())
    }
}
fn record() -> ConsentRecord {
    let mut r = ConsentRecord::default();
    r.terms = Some(DocumentReceipt::new("test", b"abcdef", 1));
    r.privacy = Some(DocumentReceipt::new("test", b"abcdef", 1));
    r.open_source_licenses = Some(DocumentReceipt::new("test", b"abcdef", 1));
    for c in DataCategory::ALL {
        r.set_permission(c, c == DataCategory::CrashReports, "test", 1);
    }
    assert!(r.complete());
    r
}
fn unpublished(target: &Disk) {
    let mut b = [0; 512];
    target.read_at(512, &mut b).unwrap();
    assert_eq!(b, [0; 512]);
    target.read_at(target.byte_len() - 512, &mut b).unwrap();
    assert_eq!(b, [0; 512]);
}
#[test]
fn real_image_installs_with_private_consent_and_last_gpt_publication() {
    let root = Temp::new();
    let disk = Disk::new(&root, fixture().bytes + 4 * 1024 * 1024);
    let mut d = Deployment::new(source(), disk.clone(), rng()).unwrap();
    assert!(d.finish(&mut |_| {}).is_err());
    assert_eq!(disk.writes.load(Ordering::Relaxed), 0);
    let mut progress = 0;
    d.copy(&mut |_| progress += 1).unwrap();
    assert!(progress > 3);
    unpublished(&disk);
    let consent = record();
    d.save_consent(&consent).unwrap();
    unpublished(&disk);
    d.finish(&mut |_| {}).unwrap();
    let mut file = File::open(root.0.join("target.img")).unwrap();
    let gpt = Gpt::load(&mut file, disk.bytes / 512).unwrap();
    assert_eq!(gpt.layout, source().gpt.layout);
    assert_ne!(gpt.primary[56..72], source().gpt.primary[56..72]);
    assert_eq!(d.stage, Stage::Complete);
    // The complete target Data remains consistent after real ext4 metadata writes.
    let data = root.0.join("installed-data.img");
    let mut output = File::create(&data).unwrap();
    file.seek(SeekFrom::Start(gpt.layout.data.first_lba * 512))
        .unwrap();
    io::copy(&mut file.take(gpt.layout.data.sectors() * 512), &mut output).unwrap();
    output.sync_all().unwrap();
    drop(output);
    run(Command::new("e2fsck").args(["-f", "-n"]).arg(data));
    assert!(d.copy(&mut |_| {}).is_err());
    assert!(d.save_consent(&consent).is_err());
}
#[test]
fn small_targets_and_entropy_failure_never_write() {
    let a = Temp::new();
    let small = Disk::new(&a, fixture().bytes - 512);
    assert!(Deployment::new(source(), small.clone(), rng()).is_err());
    assert_eq!(small.writes.load(Ordering::Relaxed), 0);
    let b = Temp::new();
    let disk = Disk::new(&b, fixture().bytes);
    assert!(Deployment::new(source(), disk.clone(), |_| Err(error("No entropy"))).is_err());
    assert_eq!(disk.writes.load(Ordering::Relaxed), 0);
}
#[test]
fn failed_flush_or_readback_never_publishes_gpt_or_allows_retry() {
    for mode in 0..3 {
        let root = Temp::new();
        let disk = Disk::new(&root, fixture().bytes);
        if mode < 2 {
            disk.fail_flush
                .store(if mode == 0 { 1 } else { 2 }, Ordering::Relaxed);
        } else {
            disk.corrupt_reads.store(true, Ordering::Relaxed);
        }
        let mut d = Deployment::new(source(), disk.clone(), rng()).unwrap();
        assert!(d.copy(&mut |_| {}).is_err());
        disk.corrupt_reads.store(false, Ordering::Relaxed);
        unpublished(&disk);
        assert!(d.finish(&mut |_| {}).is_err());
        assert!(d.copy(&mut |_| {}).is_err());
    }
}
#[test]
fn missing_consent_and_consent_flush_failure_never_publish_gpt() {
    let root = Temp::new();
    let disk = Disk::new(&root, fixture().bytes);
    let mut d = Deployment::new(source(), disk.clone(), rng()).unwrap();
    d.copy(&mut |_| {}).unwrap();
    assert!(d.finish(&mut |_| {}).is_err());
    unpublished(&disk);
    disk.fail_flush
        .store(disk.flushes.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
    assert!(d.save_consent(&record()).is_err());
    assert!(d.finish(&mut |_| {}).is_err());
    unpublished(&disk);
}
#[test]
fn source_signature_release_and_checksum_are_checked_before_writes() {
    let f = fixture();
    assert!(Source::open(&f.root.0, "wrong", 42, &[f.key.verifying_key().to_bytes()]).is_err());
    assert!(Source::open(&f.root.0, "26.0.0", 41, &[f.key.verifying_key().to_bytes()]).is_err());
    assert!(Source::open(&f.root.0, "26.0.0", 42, &[[0; 32]]).is_err());
    let root = Temp::new();
    fs::copy(f.root.0.join("disk.manifest"), root.0.join("disk.manifest")).unwrap();
    fs::copy(f.root.0.join("disk.img"), root.0.join("disk.img")).unwrap();
    let mut bad = fs::OpenOptions::new()
        .write(true)
        .open(root.0.join("disk.img"))
        .unwrap();
    bad.seek(SeekFrom::Start(100 * 512)).unwrap();
    bad.write_all(&[9]).unwrap();
    bad.sync_all().unwrap();
    assert!(Source::open(&root.0, "26.0.0", 42, &[f.key.verifying_key().to_bytes()]).is_err());
}

#[test]
fn changed_source_after_verification_is_not_published() {
    let f = fixture();
    let source_root = Temp::new();
    fs::copy(f.root.0.join("disk.img"), source_root.0.join("disk.img")).unwrap();
    fs::copy(
        f.root.0.join("disk.manifest"),
        source_root.0.join("disk.manifest"),
    )
    .unwrap();
    let source = Source::open(
        &source_root.0,
        "26.0.0",
        42,
        &[f.key.verifying_key().to_bytes()],
    )
    .unwrap();
    let mut writer = fs::OpenOptions::new()
        .write(true)
        .open(source_root.0.join("disk.img"))
        .unwrap();
    writer.seek(SeekFrom::Start(100 * 512)).unwrap();
    writer.write_all(&[17]).unwrap();
    writer.sync_all().unwrap();
    let root = Temp::new();
    let disk = Disk::new(&root, f.bytes);
    let mut d = Deployment::new(source, disk.clone(), rng()).unwrap();
    assert!(d.copy(&mut |_| {}).is_err());
    unpublished(&disk);
    assert!(d.finish(&mut |_| {}).is_err());
}
#[test]
fn cached_partition_metadata_is_bound_to_hashed_bytes() {
    let mut s = source();
    let bytes = s.bytes(0, 34 * 512).unwrap();
    assert!(s.gpt.check_chunk(0, &bytes).is_ok());
    s.gpt.primary[56] ^= 1;
    assert!(s.gpt.check_chunk(0, &bytes).is_err());
}
#[test]
fn failed_write_and_final_flush_cannot_report_complete() {
    let f = fixture();
    let root = Temp::new();
    let disk = Disk::new(&root, f.bytes);
    disk.fail_write.store(5, Ordering::Relaxed);
    let mut d = Deployment::new(source(), disk.clone(), rng()).unwrap();
    assert!(d.copy(&mut |_| {}).is_err());
    unpublished(&disk);
    let root = Temp::new();
    let disk = Disk::new(&root, f.bytes);
    let mut d = Deployment::new(source(), disk.clone(), rng()).unwrap();
    d.copy(&mut |_| {}).unwrap();
    d.save_consent(&record()).unwrap();
    disk.fail_flush
        .store(disk.flushes.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
    assert!(d.finish(&mut |_| {}).is_err());
    unpublished(&disk);
    assert!(d.stage == Stage::Failed);
    assert!(d.finish(&mut |_| {}).is_err());
}
