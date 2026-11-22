//! Persistent request policy2600f8 and completion25f350, host backend for the
//! asynchronous64KiB Wii save job2614dc..2617e8. No original saves are opened.
use super::{checkpoint::Snapshot, save_game};
use anyhow::{ensure, Context, Result};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
};

#[derive(Clone, Debug)]
struct Slot {
    header: Vec<u8>,
}
impl Slot {
    fn empty() -> Result<Self> {
        let mut header = vec![0; save_game::HEADER_SIZE];
        save_game::initialize_header(&mut header)?;
        Ok(Self { header })
    }
    ///25f970 copies fields, not the entire padded header object. Destination
    /// padding at c,2b1 and30e retains its previous allocation contents. The
    /// first41-word-pair loop includes the otherwise unowned154..158 bytes.
    fn capture(&mut self, source: &[u8]) -> Result<()> {
        ensure!(
            source.len() >= save_game::HEADER_SIZE,
            "Truncated save-slot metadata"
        );
        for range in [0..0xc, 0x10..0x2b1, 0x2b4..0x30e, 0x310..0x320] {
            self.header[range.clone()].copy_from_slice(&source[range]);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Admission {
    Started,
    Deferred,
    Declined,
}
#[derive(Debug)]
struct Job {
    result: Mutex<Option<std::result::Result<(), String>>>,
    ready: Condvar,
}
impl Job {
    fn wait(&self) -> Result<()> {
        let mut result = self
            .result
            .lock()
            .map_err(|_| anyhow::anyhow!("Save completion lock poisoned"))?;
        while result.is_none() {
            result = self
                .ready
                .wait(result)
                .map_err(|_| anyhow::anyhow!("Save completion lock poisoned"))?;
        }
        Ok(())
    }
}
/// Save slots belong to the application. A level import and shutdown can
/// observe the same frontend state and in-flight job while no world is active.
#[derive(Clone, Debug)]
pub struct Service(Arc<Mutex<Writer>>);

impl Service {
    pub fn open(root: PathBuf, selected: usize, initial_tick: u64) -> Result<Self> {
        Ok(Self(Arc::new(Mutex::new(Writer::open(root, selected, initial_tick)?))))
    }

    fn writer(&self) -> std::sync::MutexGuard<'_, Writer> {
        // Only a level loader can unwind through a save operation. Its staged
        // world is discarded, but the application must still finish the job.
        self.0.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub fn read_checkpoint(root: &Path, selected: usize) -> Result<Arc<Snapshot>> {
        Writer::read_checkpoint(root, selected)
    }

    #[cfg(feature = "blotworx-ui-render")]
    pub(super) fn delete_confirmed(root: &Path, slot: usize, expected: &Snapshot) -> Result<()> {
        let current = Self::read_checkpoint(root, slot)?;
        ensure!(current.bytes == expected.bytes, "Save slot changed during confirmation");
        let path = root.join(format!("save{slot}.dat"));
        std::fs::remove_file(&path).with_context(|| format!("Delete checkpoint {}", path.display()))
    }

    pub(super) fn read_selected_checkpoint(&self) -> Result<Option<Arc<Snapshot>>> {
        self.writer().read_selected_checkpoint()
    }

    pub fn blocks_memory_capture(&self) -> bool {
        let writer = self.writer();
        writer.closed || writer.blocks_memory_capture()
    }

    pub fn pending(&self) -> bool { self.writer().pending() }
    pub fn writes(&self) -> u64 { self.writer().writes() }
    pub fn summary(&self) -> serde_json::Value { self.writer().summary() }

    pub fn record_memory_checkpoint(&self, snapshot: &Arc<Snapshot>) {
        let mut writer = self.writer();
        if !writer.closed {
            // REL25fb00/25ddc8 attach the common allocation at +988 even
            // for memory-only captures. A later forced job uses that buffer.
            writer.memory_checkpoint = Some(snapshot.clone());
        }
    }

    pub fn request(&self, snapshot: &Arc<Snapshot>, now: u64, frequency: u32) -> Result<Admission> {
        let mut writer = self.writer();
        if writer.closed {
            return Ok(Admission::Declined);
        }
        writer.request(snapshot, now, frequency, false)
    }

    /// The completion write bypasses the ordinary checkpoint throttle.
    pub(super) fn complete_playthrough(&self, snapshot: &Arc<Snapshot>) -> Result<Admission> {
        let mut writer = self.writer();
        if writer.closed || writer.disabled {
            return Ok(Admission::Declined);
        }
        if let Some(job) = &writer.job {
            job.wait()?;
            writer.poll()?;
        }
        let selected = writer.selected;
        writer.slots[selected].capture(&snapshot.bytes)?;
        let admission = writer.start_write(snapshot)?;
        writer.memory_checkpoint = Some(snapshot.clone());
        Ok(admission)
    }

    pub fn poll(&self) -> Result<()> { self.writer().poll() }

    pub fn flush(&self, now: u64, frequency: u32) -> Result<()> {
        self.writer().flush_on_exit(now, frequency)
    }

    pub fn shutdown(&self, now: u64, frequency: u32) -> Result<()> {
        let mut writer = self.writer();
        writer.closed = true;
        writer.flush_on_exit(now, frequency)
    }
}

#[derive(Debug)]
struct Writer {
    root: PathBuf,
    selected: usize,
    slots: [Slot; 3],
    last_write_tick: u64,
    pending: bool,
    frontend: u32,
    status: u32,
    disabled: bool,
    job: Option<Arc<Job>>,
    writes: u64,
    last_error: Option<String>,
    memory_checkpoint: Option<Arc<Snapshot>>,
    closed: bool,
}
impl Writer {
    pub fn read_checkpoint(root: &Path, selected: usize) -> Result<Arc<Snapshot>> {
        ensure!(selected < 3, "Save slot must be 0, 1 or 2");
        let path = root.join(format!("save{selected}.dat"));
        let bytes = std::fs::read(&path)
            .with_context(|| format!("Read checkpoint {}", path.display()))?;
        ensure!(bytes.len() == 0x10000, "Invalid checkpoint storage size");
        let logical_size = save_game::Archive::parse(&bytes)?.logical_size;
        Ok(Arc::new(Snapshot { bytes, logical_size }))
    }

    pub(super) fn read_selected_checkpoint(&self) -> Result<Option<Arc<Snapshot>>> {
        match Self::read_checkpoint(&self.root, self.selected) {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(error) if error.downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn open(root: PathBuf, selected: usize, initial_tick: u64) -> Result<Self> {
        ensure!(selected < 3, "Save slot must be0,1 or2");
        std::fs::create_dir_all(&root)
            .with_context(|| format!("Create Blotworx save directory {}", root.display()))?;
        let mut slots = [Slot::empty()?, Slot::empty()?, Slot::empty()?];
        for (index, slot) in slots.iter_mut().enumerate() {
            let path = root.join(format!("save{index}.dat"));
            match std::fs::read(&path) {
                Ok(bytes) => {
                    ensure!(
                        bytes.len() == 0x10000,
                        "Blotworx save slot{index} has an invalid storage size"
                    );
                    save_game::Archive::parse(&bytes)
                        .with_context(|| format!("Validate Blotworx save slot{index}"))?;
                    slot.capture(&bytes)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| format!("Read Blotworx save slot{index}"))
                }
            }
        }
        Ok(Self {
            root,
            selected,
            slots,
            last_write_tick: u64::from(initial_tick as u32),
            pending: false,
            frontend: 0,
            status: 0,
            disabled: false,
            job: None,
            writes: 0,
            last_error: None,
            memory_checkpoint: None,
            closed: false,
        })
    }
    pub fn blocks_memory_capture(&self) -> bool {
        matches!(self.frontend, 4 | 5 | 6)
    }
    pub fn pending(&self) -> bool {
        self.pending
    }
    pub fn writes(&self) -> u64 {
        self.writes
    }
    ///2601bc uses signed64 subtraction/division, then cmpwi on the LOW result
    /// word. Preserve the low-word signed comparison rather than an f64 timer.
    fn throttled(&self, now: u64, frequency: u32) -> bool {
        let seconds = (now.wrapping_sub(self.last_write_tick) as i64) / i64::from(frequency);
        (seconds as i32) < 240
    }
    pub fn request(
        &mut self,
        snapshot: &Arc<Snapshot>,
        now: u64,
        frequency: u32,
        force: bool,
    ) -> Result<Admission> {
        ensure!(frequency != 0, "Save timebase frequency is zero");
        if self.disabled {
            return Ok(Admission::Declined);
        }
        if self.blocks_memory_capture() {
            return Ok(Admission::Declined);
        }
        ensure!(
            snapshot.bytes.len() == 0x10000,
            "Persistent save needs the native64KiB allocation"
        );
        save_game::Archive::parse(&snapshot.bytes)
            .context("Refuse incomplete persistent checkpoint")?;
        //26013c copies the header BEFORE testing the240-second throttle.
        if !force {
            self.slots[self.selected].capture(&snapshot.bytes)?;
            self.memory_checkpoint = Some(snapshot.clone());
        }
        if force {
            if !self.pending {
                return Ok(Admission::Declined);
            }
        } else if self.throttled(now, frequency) {
            self.pending = true;
            return Ok(Admission::Deferred);
        }
        let admission = self.start_write(snapshot)?;
        self.last_write_tick = now;
        Ok(admission)
    }

    fn start_write(&mut self, snapshot: &Arc<Snapshot>) -> Result<Admission> {
        let job = Arc::new(Job {
            result: Mutex::new(None),
            ready: Condvar::new(),
        });
        let completion = job.clone();
        let root = self.root.clone();
        let selected = self.selected;
        let snapshot = snapshot.clone();
        std::thread::Builder::new()
            .name("Blotworx checkpoint".into())
            .spawn(move || {
                let result = write_slot(&root, selected, &snapshot.bytes)
                    .map_err(|error| format!("{error:#}"));
                if let Ok(mut slot) = completion.result.lock() {
                    *slot = Some(result);
                    completion.ready.notify_all();
                }
            })
            .context("Start persistent checkpoint job")?;
        self.pending = false;
        self.frontend = 6;
        self.status = 2;
        self.last_error = None;
        self.job = Some(job);
        Ok(Admission::Started)
    }
    /// Called independently of gameplay pause. Completion is observed only
    /// after the actual host write/sync/close/commit has succeeded or failed.
    pub fn poll(&mut self) -> Result<()> {
        let Some(job) = &self.job else { return Ok(()) };
        let result = job
            .result
            .lock()
            .map_err(|_| anyhow::anyhow!("Save completion lock poisoned"))?
            .clone();
        let Some(result) = result else { return Ok(()) };
        self.job = None;
        match result {
            Ok(()) => {
                self.frontend = 0;
                self.status = 5;
                self.writes += 1;
                Ok(())
            }
            Err(error) => {
                self.frontend = 0;
                self.pending = true;
                self.last_error = Some(error.clone());
                anyhow::bail!("Persistent checkpoint failed: {error}")
            }
        }
    }
    pub fn flush_on_exit(
        &mut self,
        now: u64,
        frequency: u32,
    ) -> Result<()> {
        if let Some(job) = &self.job {
            job.wait()?;
            self.poll()?;
        }
        if self.pending {
            let snapshot = self.memory_checkpoint.clone().context("Pending save has no memory checkpoint")?;
            self.request(&snapshot, now, frequency, true)?;
            if let Some(job) = &self.job {
                job.wait()?;
                self.poll()?;
            }
        }
        Ok(())
    }
    pub fn summary(&self) -> serde_json::Value {
        let slots = self
            .slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let string = |offset: usize| {
                    let bytes = &slot.header[offset..offset + 60];
                    String::from_utf8_lossy(
                        &bytes[..bytes.iter().position(|&byte| byte == 0).unwrap_or(60)],
                    )
                    .into_owned()
                };
                serde_json::json!({"slot":index,"level":string(0x10),"callback":string(0xc4)})
            })
            .collect::<Vec<_>>();
        serde_json::json!({"directory":self.root,"selected_slot":self.selected,"slots":slots,
            "frontend_state":self.frontend,"status":self.status,"pending_throttled_write":self.pending,
            "completed_writes":self.writes,"last_write_tick_hex":format!("0x{:016x}",self.last_write_tick),
            "last_error":self.last_error,"backend":"Blotworx host filesystem",
            "disabled":self.disabled,"native_service_bootstrap_verified":false,
            "native_io_timing_verified":false,"native_frontend_error_movies_implemented":false,
            "gameplay_restore_implemented":false})
    }
}
pub fn default_directory() -> Result<PathBuf> {
    #[cfg(target_os = "android")]
    let storage = crate::android_app::storage();
    #[cfg(target_os = "horizon")]
    let storage = Some(PathBuf::from("sdmc:/blotworx"));
    #[cfg(not(any(target_os = "android", target_os = "horizon")))]
    let storage = dirs::data_local_dir();
    Ok(storage
        .context("Local application storage is unavailable")?
        .join("Blotworx Engine")
        .join("saves"))
}

/// Host equivalent of the original safe-open/write64KiB/close job. Keep one
/// previous committed file. A failed write never truncates the committed save.
fn write_slot(root: &Path, selected: usize, bytes: &[u8]) -> Result<()> {
    let destination = root.join(format!("save{selected}.dat"));
    let temporary = root.join(format!(".save{selected}.{}.tmp", std::process::id()));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .with_context(|| format!("Create temporary checkpoint {}", temporary.display()))?;
    struct Temporary(PathBuf);
    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = Temporary(temporary.clone());
    // Declare the handle after its guard so errors close it before cleanup,
    // including on hosts that forbid deleting an open file.
    let mut file = file;
    file.write_all(bytes)
        .context("Write checkpoint allocation")?;
    file.sync_all().context("Sync checkpoint allocation")?;
    drop(file);
    if destination.exists() {
        let backup = root.join(format!("save{selected}.dat.bak"));
        std::fs::copy(&destination, &backup).context("Preserve previous Blotworx checkpoint")?;
    }
    std::fs::rename(&temporary, &destination).context("Commit Blotworx checkpoint")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(
                format!("blotworx-save-{}-{}",std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()),
            ))
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            for index in 0..3 {
                for name in [format!("save{index}.dat"), format!("save{index}.dat.bak")] {
                    let _ = std::fs::remove_file(self.0.join(name));
                }
            }
            let _ = std::fs::remove_dir(&self.0);
        }
    }
    fn snapshot(marker: u8) -> Result<Arc<Snapshot>> {
        let mut bytes = vec![0; 0x10000];
        save_game::initialize_header(&mut bytes)?;
        bytes[0x10] = marker;
        bytes[0x2c4] = 0;
        let built = save_game::MemoryWriter::from_storage(bytes)?.finish()?;
        Ok(Arc::new(Snapshot {
            bytes: built.bytes,
            logical_size: built.logical_size,
        }))
    }
    #[test]
    fn admission_and_slot_metadata_match_bounded_original_instructions() -> Result<()> {
        let reference: serde_json::Value = serde_json::from_str(include_str!(
            "../../tools/blotworx/fixtures/save_service.json"
        ))?;
        assert_eq!(
            reference["rel_sha256"],
            "ac25d520f329a3d0507d656f0ab1cf1c34025bb36ff866187ad082ae3e0a4ebf"
        );
        for case in reference["cases"]
            .as_array()
            .context("Missing native admission cases")?
        {
            let directory = Directory::new();
            let mut service = Writer::open(directory.0.clone(), 0, 0)?;
            service.last_write_tick = case["last"].as_u64().unwrap();
            service.pending = case["pending"].as_bool().unwrap();
            service.frontend = case["frontend"].as_u64().unwrap() as u32;
            service.disabled = case["disabled"].as_bool().unwrap();
            let snapshot = snapshot(b'E')?;
            let now = case["now"].as_u64().unwrap();
            service.request(&snapshot, now, 1000, case["force"].as_bool().unwrap())?;
            assert_eq!(
                u32::from(service.pending),
                case["after_pending"].as_u64().unwrap() as u32,
                "{}",
                case["name"]
            );
            assert_eq!(
                service.frontend,
                case["after_frontend"].as_u64().unwrap() as u32,
                "{}",
                case["name"]
            );
            assert_eq!(
                service.last_write_tick,
                case["after_last"].as_u64().unwrap(),
                "{}",
                case["name"]
            );
            assert_eq!(
                service.status,
                case["after_status"].as_u64().unwrap() as u32,
                "{}",
                case["name"]
            );
            if let Some(job) = &service.job {
                job.wait()?;
                service.poll()?;
            }
        }
        let mut slot = Slot {
            header: (0..0x320).map(|i| (i * 19 + 7) as u8).collect(),
        };
        let source = (0..0x320).map(|i| (i * 37 + 11) as u8).collect::<Vec<_>>();
        slot.capture(&source)?;
        let hash = ring::digest::digest(&ring::digest::SHA256, &slot.header)
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(hash, reference["slot_copy"]["sha256"].as_str().unwrap());
        Ok(())
    }
    #[test]
    fn real_async_writes_keep_native_allocation_throttle_metadata_and_force_on_quit() -> Result<()>
    {
        let directory = Directory::new();
        let tick = 600_000_000_000u64;
        let mut service = Writer::open(directory.0.clone(), 0, tick)?;
        let first = snapshot(b'A')?;
        assert_eq!(service.last_write_tick, u64::from(tick as u32));
        assert_eq!(
            service.request(&first, tick, 1000, false)?,
            Admission::Started
        );
        assert!(service.blocks_memory_capture());
        assert_eq!(
            service.request(&first, tick, 1000, true)?,
            Admission::Declined
        );
        service.flush_on_exit(tick, 1000)?;
        assert_eq!(service.writes(), 1);
        assert_eq!(std::fs::read(directory.0.join("save0.dat"))?, first.bytes);
        service.slots[0].header[0xc] = 0xa5;
        let second = snapshot(b'B')?;
        assert_eq!(
            service.request(&second, tick + 239999, 1000, false)?,
            Admission::Deferred
        );
        assert!(service.pending());
        assert_eq!(service.slots[0].header[0x10], b'B');
        assert_eq!(service.slots[0].header[0xc], 0xa5);
        assert_eq!(std::fs::read(directory.0.join("save0.dat"))?, first.bytes);
        service.flush_on_exit(tick + 239999, 1000)?;
        assert_eq!(service.writes(), 2);
        assert!(!service.pending());
        assert_eq!(std::fs::read(directory.0.join("save0.dat"))?, second.bytes);
        assert_eq!(
            std::fs::read(directory.0.join("save0.dat.bak"))?,
            first.bytes
        );
        assert_eq!(
            service.request(&second, tick + 500000, 1000, true)?,
            Admission::Declined
        );
        let reopened = Writer::open(directory.0.clone(), 0, tick)?;
        assert_eq!(reopened.slots[0].header[0x10], b'B');
        Ok(())
    }
    #[test]
    fn throttle_uses_signed_low_quotient_word_and_exact_boundary() -> Result<()> {
        let directory = Directory::new();
        let mut service = Writer::open(directory.0.clone(), 2, 0)?;
        let snapshot = snapshot(b'C')?;
        assert_eq!(
            service.request(&snapshot, 239999, 1000, false)?,
            Admission::Deferred
        );
        assert_eq!(
            service.request(&snapshot, 240000, 1000, false)?,
            Admission::Started
        );
        service.flush_on_exit(240000, 1000)?;
        assert!(!directory.0.join("save0.dat").exists());
        assert!(directory.0.join("save2.dat").is_file());
        service.last_write_tick = 0;
        assert!(service.throttled((u64::from(u32::MAX) + 1 + 239) * 1000, 1000));
        assert!(!service.throttled((u64::from(u32::MAX) + 1 + 240) * 1000, 1000));
        assert!(service.throttled(u64::MAX - 100, 1000));
        Ok(())
    }
    #[test]
    fn failed_host_job_reports_error_and_preserves_committed_file() -> Result<()> {
        let directory = Directory::new();
        let mut service = Writer::open(directory.0.clone(), 0, 0)?;
        let snapshot = snapshot(b'D')?;
        std::fs::write(
            directory.0.join("save0.dat"),
            b"previous committed checkpoint",
        )?;
        let collision = directory
            .0
            .join(format!(".save0.{}.tmp", std::process::id()));
        std::fs::write(&collision, b"occupied")?;
        assert_eq!(
            service.request(&snapshot, 240000, 1000, false)?,
            Admission::Started
        );
        let result = service.flush_on_exit(240000, 1000);
        assert!(result.is_err());
        assert!(service.last_error.is_some());
        assert!(service.pending());
        assert_eq!(
            std::fs::read(directory.0.join("save0.dat"))?,
            b"previous committed checkpoint"
        );
        assert_eq!(std::fs::read(&collision)?, b"occupied");
        std::fs::remove_file(collision)?;
        Ok(())
    }
    #[test]
    fn completed_playthrough_supersedes_a_throttled_write() -> Result<()> {
        let directory = Directory::new();
        let service = Service::open(directory.0.clone(), 1, 0)?;
        let active = snapshot(b'A')?;
        assert_eq!(service.request(&active, 0, 1000)?, Admission::Deferred);
        let mut bytes = active.bytes.clone();
        bytes[0x2c4] = 1;
        let completed = Arc::new(Snapshot { bytes, logical_size: active.logical_size });
        assert_eq!(service.complete_playthrough(&completed)?, Admission::Started);
        service.flush(0, 1000)?;
        assert_eq!(service.writes(), 1);
        assert!(!service.pending());
        assert_eq!(std::fs::read(directory.0.join("save1.dat"))?, completed.bytes);
        Ok(())
    }

    #[test]
    fn shutdown_during_level_import_commits_the_live_memory_buffer_once() -> Result<()> {
        let directory = Directory::new();
        let application = Service::open(directory.0.clone(), 2, 0)?;
        let departing_world = application.clone();
        let incoming_world = departing_world.clone();
        assert_eq!(departing_world.request(&snapshot(b'A')?, 1000, 1000)?, Admission::Deferred);
        drop(departing_world);
        let latest = snapshot(b'B')?;
        incoming_world.record_memory_checkpoint(&latest);
        assert!(application.pending());
        assert!(application.read_selected_checkpoint()?.is_none());

        application.shutdown(1000, 1000)?;
        assert_eq!(incoming_world.writes(), 1);
        assert!(incoming_world.blocks_memory_capture());
        assert_eq!(incoming_world.request(&snapshot(b'C')?, 250000, 1000)?, Admission::Declined);
        incoming_world.record_memory_checkpoint(&snapshot(b'C')?);
        application.shutdown(250000, 1000)?;
        assert_eq!(application.writes(), 1);
        assert_eq!(std::fs::read(directory.0.join("save2.dat"))?, latest.bytes);
        Ok(())
    }

    #[test]
    #[ignore = "requires original EM1 files in BLOTWORX_TEST_DATA"]
    fn original_gv_entrance_dispatches_real_persistent_checkpoint_without_replacing_script(
    ) -> Result<()> {
        let root = std::env::var_os("BLOTWORX_TEST_DATA").context("Set BLOTWORX_TEST_DATA")?;
        let directory = Directory::new();
        let context = super::super::frame_clock::Context::boot(Some(600_000_000));
        let service = Service::open(directory.0.clone(), 0, context.hardware_sample())?;
        let checkpoint = super::super::checkpoint::State {
            service: Some(service),
            ..Default::default()
        };
        let mut world = super::super::assets::load_with_score_state(
            std::path::Path::new(&root),
            "Levels/GV_ZoneA_Start.level",
            &egui::Context::default(),
            None,
            None,
            Default::default(),
            Default::default(),
            super::super::score_state::InitialState {
                checkpoint,
                frame_clock: Some(context),
                ..Default::default()
            },
        )?;
        for tick in 1..=150 {
            world.frame_clock.publish(true, true);
            world.advance_models(tick as f32 / 30.)?;
            world.checkpoint.service.as_mut().unwrap().poll()?;
        }
        let snapshot = world
            .checkpoint
            .latest
            .clone()
            .context("Original entrance did not request a checkpoint")?;
        let archive = save_game::Archive::parse(&snapshot.bytes)?;
        assert_eq!(archive.header_string(0xc4, 60)?, "GV_ZoneA_LoadCheckPoint");
        assert_eq!(archive.sections.len(), 10);
        assert!(world.checkpoint.notification.is_some());
        assert!(
            !world
                .report
                .unsupported
                .iter()
                .any(|message| message.contains("SaveGameToMemory")),
            "{:?}",
            world.report.unsupported
        );
        let service = world.checkpoint.service.as_mut().unwrap();
        service.flush(
            world.frame_clock.hardware_sample(),
            world.frame_clock.clock.rate.ticks_per_second.get(),
        )?;
        assert_eq!(service.writes(), 1);
        assert_eq!(
            std::fs::read(directory.0.join("save0.dat"))?,
            snapshot.bytes
        );
        Ok(())
    }
}
