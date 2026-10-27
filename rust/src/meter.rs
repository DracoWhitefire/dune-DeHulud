//! Finds the game, identifies its build and walks the threat route once per poll (read-only).

use crate::help;
use crate::procfs::{self, Memory, Source};
use std::time::{Duration, Instant};

pub const APP: &str = "DeHulud";
pub const EXE: &str = "DuneSandbox-Win64-Shipping.exe";
/// 4 polls per second.
pub const POLL: Duration = Duration::from_millis(250);
/// While no game is attached, look for it every 2 seconds.
pub const SCAN: Duration = Duration::from_secs(2);
/// Type checks: ClassPrivate, then at most 4 SuperStruct steps.
pub const MAX_DEPTH: usize = 4;
/// Accepted range of the two threat values: [0, 1e6].
pub const VALUE_MAX: f64 = 1_000_000.0;
/// The PE header is identified from the first 0x400 bytes of the image when the game is found; each poll
/// re-reads its start up to SizeOfImage (see `walk`).
pub const HEADER: usize = 0x400;
/// Window hint: the full message is in the right-click menu (and on stderr).
pub const MENU_HINT: &str = "right-click";

/// One entry of the builds table. Key = (PE TimeDateStamp, SizeOfImage) as read from the game's mapped PE
/// header. Offsets come from static analysis of that build's executable; each is commented with the game
/// member it reads. RVA values are relative to the image base; the rest are member offsets.
#[derive(Debug)]
pub struct Build {
    pub build: &'static str,
    pub pe_timestamp: u32,
    pub size_of_image: u32,
    pub gworld: u64,
    pub cls_world: u64,
    pub cls_game_instance: u64,
    pub cls_local_player: u64,
    pub cls_player_controller: u64,
    pub cls_dune_character: u64,
    pub cls_dune_player_character: u64,
    pub cls_dune_vehicle: u64,
    pub cls_threat_component: u64,
    pub class_private: u64,
    pub super_struct: u64,
    pub world_game_instance: u64,
    pub local_players: u64,
    pub array_data: usize,
    pub array_num: usize,
    pub player_controller: u64,
    pub pawn: u64,
    pub acknowledged_pawn: u64,
    pub character_threat: u64,
    pub vehicle_threat: u64,
    pub threat_blob: u64,
    pub threat_threshold: u64,
}

/// Builds table. Build 26083551: pe_timestamp 1792504044, size_of_image 220008448 (executable 214900656
/// bytes).
pub static BUILDS: &[Build] = &[Build {
    build: "26083551",
    pe_timestamp: 1792504044,
    size_of_image: 220008448,
    gworld: 0xC4CF528,                    // GWorld slot RVA (holds the UWorld*)
    cls_world: 0xC4CEF00,                 // UWorld class slot RVA (holds the UClass*)
    cls_game_instance: 0xC49DBB0,         // UGameInstance class slot RVA
    cls_local_player: 0xC4AD730,          // ULocalPlayer class slot RVA
    cls_player_controller: 0xC4BB0E8,     // APlayerController class slot RVA
    cls_dune_character: 0xC166430,        // ADuneCharacter class slot RVA
    cls_dune_player_character: 0xC16F290, // ADunePlayerCharacter class slot RVA
    cls_dune_vehicle: 0xC1E3D68,          // ADuneVehicle class slot RVA
    cls_threat_component: 0xC1D8BD8,      // USandwormThreatSourceComponent class slot RVA
    class_private: 0x10,                  // UObject.ClassPrivate
    super_struct: 0x40,                   // UStruct.SuperStruct
    world_game_instance: 0x3C8,           // UWorld.OwningGameInstance
    local_players: 0x38,                  // UGameInstance.LocalPlayers (TArray)
    array_data: 0x0,                      // TArray.Data (element stride 8)
    array_num: 0x8,                       // TArray.Num (i32)
    player_controller: 0x30,              // ULocalPlayer.PlayerController
    pawn: 0x388,                          // AController.Pawn (the possessed pawn: decides the route)
    acknowledged_pawn: 0x3F0,             // APlayerController.AcknowledgedPawn (on foot: must equal Pawn)
    character_threat: 0xE88,              // ADuneCharacter.m_SandwormThreatSourceComponent
    vehicle_threat: 0xE08,                // ADuneVehicle.m_SandwormThreatSourceComponent
    threat_blob: 0x334,                   // m_CurrentThreatBlobValue (f32)
    threat_threshold: 0x348,              // m_CurrentSandwormAttackThreshold (f32)
}];

// Class slots, in this order (index = bit in a hit mask).
const WORLD: usize = 0;
const GAME_INSTANCE: usize = 1;
const LOCAL_PLAYER: usize = 2;
const PLAYER_CONTROLLER: usize = 3;
const DUNE_CHARACTER: usize = 4;
const DUNE_PLAYER_CHARACTER: usize = 5;
const DUNE_VEHICLE: usize = 6;
const THREAT_COMPONENT: usize = 7;
/// On foot: the pawn must be BOTH character classes.
const CHARACTER: u8 = 1 << DUNE_CHARACTER | 1 << DUNE_PLAYER_CHARACTER;

impl Build {
    fn class_slots(&self) -> [u64; 8] {
        [
            self.cls_world,
            self.cls_game_instance,
            self.cls_local_player,
            self.cls_player_controller,
            self.cls_dune_character,
            self.cls_dune_player_character,
            self.cls_dune_vehicle,
            self.cls_threat_component,
        ]
    }
}

/// The builds table entry for a PE identity.
pub fn find_build(ident: (u32, u32)) -> Option<&'static Build> {
    BUILDS.iter().find(|b| (b.pe_timestamp, b.size_of_image) == ident)
}

/// One poll: `percent`/`fraction` are None whenever the meter must show '-'.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub status: String,
    pub percent: Option<u32>,
    pub fraction: Option<f64>,
    pub detail: String,
    pub hint: String,
}

impl Reading {
    pub fn new(status: impl Into<String>) -> Self {
        Reading { status: status.into(), percent: None, fraction: None, detail: String::new(), hint: String::new() }
    }

    pub fn with(status: impl Into<String>, detail: impl Into<String>, hint: impl Into<String>) -> Self {
        Reading { detail: detail.into(), hint: hint.into(), ..Reading::new(status) }
    }

    pub fn value(status: impl Into<String>, percent: u32, fraction: f64) -> Self {
        Reading { percent: Some(percent), fraction: Some(fraction), ..Reading::new(status) }
    }
}

/// Anything that produces one reading per poll (the meter, or a scripted one in tests).
pub trait Poll {
    fn poll(&mut self) -> Reading;
}

/// (address, path) of the lowest mapping of a file named EXE at file offset 0, or None.
pub fn image_base(maps_text: &str) -> Option<(u64, String)> {
    let mut best: Option<(u64, String)> = None;
    for line in maps_text.split(['\n', '\r']) {
        let parts = split_fields(line, 5);
        if parts.len() < 6 {
            continue;
        }
        let path = parts[5].strip_suffix(" (deleted)").unwrap_or(parts[5]);
        let start = parts[0].split('-').next().and_then(|s| u64::from_str_radix(s, 16).ok());
        let offset = u64::from_str_radix(parts[2], 16).ok();
        let (Some(start), Some(offset)) = (start, offset) else { continue };
        let name = path.rsplit('/').next().unwrap_or(path);
        if offset == 0 && name == EXE && best.as_ref().is_none_or(|b| start < b.0) {
            best = Some((start, path.to_string()));
        }
    }
    best
}

/// Splits like Python's `str.split(None, maxsplit)`: runs of whitespace separate fields, leading whitespace
/// is skipped, and the last field is the rest of the line (trailing whitespace kept).
fn split_fields(line: &str, maxsplit: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = line.trim_start();
    while !rest.is_empty() {
        if out.len() == maxsplit {
            out.push(rest);
            break;
        }
        match rest.find(char::is_whitespace) {
            Some(i) => {
                out.push(&rest[..i]);
                rest = rest[i..].trim_start();
            }
            None => {
                out.push(rest);
                break;
            }
        }
    }
    out
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// (TimeDateStamp, SizeOfImage) of a PE32+ x64 header, or None if it is not one.
pub fn pe_identity(head: &[u8]) -> Option<(u32, u32)> {
    if head.len() < 0x40 || &head[..2] != b"MZ" {
        return None;
    }
    let pe = u32_at(head, 0x3C) as usize;
    if pe + 0x58 > head.len() || &head[pe..pe + 4] != b"PE\0\0" {
        return None;
    }
    if u16_at(head, pe + 4) != 0x8664 || u16_at(head, pe + 24) != 0x20B {
        return None;
    }
    Some((u32_at(head, pe + 8), u32_at(head, pe + 24 + 56)))
}

/// meter = clamp(blob / threshold, 0, 1); '-' for anything outside the accepted range.
pub fn reading_for(mode: &str, blob: f64, threshold: f64) -> Reading {
    if !(blob.is_finite() && (0.0..=VALUE_MAX).contains(&blob)) {
        return Reading::new(format!("{mode}, no value"));
    }
    if !(threshold.is_finite() && threshold > 0.0 && threshold <= VALUE_MAX) {
        return Reading::new(format!("{mode}, no threshold"));
    }
    let percent = (blob * 100.0 / threshold).floor().clamp(0.0, 100.0) as u32; // whole percent, floored, clamped
    Reading::value(mode, percent, (blob / threshold).clamp(0.0, 1.0))
}

/// Why a poll stops early: a soft '-' with a status word (the game stays attached), or a failed read.
#[derive(Debug, PartialEq)]
enum Fail {
    Soft(&'static str),
    Os(i32),
}

struct Game {
    pid: i32,
    base: u64,
    build: &'static Build,
    reader: Box<dyn Memory>,
    slots: Option<[u64; 8]>, // class slot values, read once per attachment
    ident: (u32, u32),       // PE identity of the attached image
    span: usize,             // header bytes re-read each poll
}

enum Walk {
    Done(Reading),
    Failed(i32),
    /// The attached process now runs another image: identify it again.
    Replaced,
}

pub type ReaderFactory = Box<dyn FnMut(i32) -> Box<dyn Memory>>;

/// Finds the game, identifies its build and walks the threat route once per [`Poll::poll`].
pub struct Meter {
    source: Box<dyn Source>,
    reader_factory: ReaderFactory,
    pub launched: bool,
    scan_interval: Duration,
    game: Option<Game>,
    /// Said with every reading of this attachment (a game found outside --launch).
    note: String,
    next_scan: Option<Instant>,
    /// Memory reads of the last poll.
    pub reads: u64,
    last: Reading,
    me: (i32, u32),
}

impl Meter {
    pub fn new(
        source: Box<dyn Source>,
        reader_factory: ReaderFactory,
        launched: bool,
        scan_interval: Duration,
    ) -> Self {
        Meter {
            source,
            reader_factory,
            launched,
            scan_interval,
            game: None,
            note: String::new(),
            next_scan: None,
            reads: 0,
            last: Reading::new("waiting for game"),
            me: procfs::me(),
        }
    }

    /// The meter of a normal run: the real /proc and process_vm_readv.
    pub fn real(launched: bool) -> Self {
        Meter::new(
            Box::new(procfs::ProcSource),
            Box::new(|pid| -> Box<dyn Memory> { Box::new(procfs::Reader::new(pid)) }),
            launched,
            SCAN,
        )
    }

    /// The build of the attached game, if any.
    pub fn attached_build(&self) -> Option<&'static str> {
        self.game.as_ref().map(|g| g.build.build)
    }

    fn poll_inner(&mut self) -> Reading {
        self.reads = 0;
        if self.game.is_none() && self.next_scan.is_some_and(|t| Instant::now() < t) {
            return self.last.clone();
        }
        for _ in 0..2 {
            // a second round only when the attached process now runs another image
            if self.game.is_none() {
                self.next_scan = Some(Instant::now() + self.scan_interval);
                if let Some(r) = self.attach() {
                    self.last = r;
                    return self.last.clone();
                }
            }
            let g = self.game.as_mut().expect("attached");
            let start = g.reader.count();
            let out = walk(&*self.source, g);
            self.reads += g.reader.count() - start;
            match out {
                Walk::Done(r) => {
                    self.last = r;
                    return self.last.clone();
                }
                Walk::Failed(code) => {
                    self.last = self.failed(code);
                    return self.last.clone();
                }
                Walk::Replaced => {
                    self.game = None;
                    self.next_scan = None;
                }
            }
        }
        self.last = Reading::new("no data");
        self.last.clone()
    }

    /// Whether pid descends from DeHulud. Only the user's own processes are read: the walk stops at the first
    /// ancestor that belongs to another user, before its status is opened.
    pub fn descends(&self, mut pid: i32) -> bool {
        let (me, uid) = self.me;
        for _ in 0..64 {
            match self.source.uid(pid) {
                Ok(u) if u == uid => {}
                _ => return false,
            }
            pid = match self.source.ppid(pid) {
                Ok(p) => p,
                Err(_) => return false,
            };
            if pid == me {
                return true;
            }
            if pid <= 1 {
                return false;
            }
        }
        false
    }

    /// Looks for the game; None when it is now attached, else what this poll shows.
    fn attach(&mut self) -> Option<Reading> {
        let (me, uid) = self.me;
        let mut found: Vec<(i32, u64)> = Vec::new();
        let mut foreign: Vec<(i32, u64)> = Vec::new();
        for pid in self.source.pids() {
            if pid == me {
                continue;
            }
            match self.source.uid(pid) {
                Ok(u) if u == uid => {}
                _ => continue,
            }
            match self.source.cmdline(pid) {
                Ok(c) if contains(&c, EXE.as_bytes()) => {}
                _ => continue,
            }
            let hit = match self.source.maps(pid) {
                Ok(m) => image_base(&m),
                Err(_) => continue,
            };
            if let Some((base, _)) = hit {
                if self.launched && !self.descends(pid) {
                    foreign.push((pid, base));
                } else {
                    found.push((pid, base));
                }
            }
        }
        self.note.clear();
        if found.is_empty() && !foreign.is_empty() {
            // Steam started the game outside this --launch DeHulud. With ptrace_scope 0 the kernel allows the read
            // anyway (so it is tried; without a readable setting too: a refusal then says why). With 1-3 a read
            // would be refused, so nothing is read and the message says what works for that value.
            let scope = self.source.ptrace_scope();
            match scope {
                Some(0) | None => {}
                Some(s) => {
                    return Some(Reading::with(
                        "not started by --launch",
                        help::outside_launch_help(foreign[0].0, s, &help::program()),
                        MENU_HINT,
                    ));
                }
            }
            self.note = help::outside_note(foreign[0].0, scope);
            found = foreign;
        }
        let Some(&(pid, base)) = found.first() else {
            return Some(Reading::new("waiting for game"));
        };
        let mut reader = (self.reader_factory)(pid);
        let head = reader.read(base, HEADER);
        self.reads += reader.count();
        let head = match head {
            Ok(h) => h,
            Err(code) => return Some(self.failed(code)),
        };
        let Some(ident) = pe_identity(&head) else {
            return Some(Reading::new("waiting for game"));
        };
        let Some(build) = find_build(ident) else {
            return Some(Reading::with(
                "Unsupported game version",
                help::unsupported(ident),
                format!("PE {}", ident.0),
            ));
        };
        let span = u32_at(&head, 0x3C) as usize + 0x58; // e_lfanew + what pe_identity checks
        self.game = Some(Game { pid, base, build, reader, slots: None, ident, span });
        None
    }

    fn failed(&mut self, code: i32) -> Reading {
        if code == libc::EPERM || code == libc::EACCES {
            self.game = None;
            return Reading::with(
                "no permission",
                help::permission_help(self.source.ptrace_scope(), self.launched, &help::program()),
                MENU_HINT,
            );
        }
        if code == libc::ESRCH {
            self.game = None;
            self.next_scan = None;
            return Reading::new("waiting for game");
        }
        Reading::new("no data") // EFAULT/EIO: a pointer went stale during this poll
    }
}

impl Poll for Meter {
    fn poll(&mut self) -> Reading {
        let mut r = self.poll_inner();
        if !self.note.is_empty() && self.game.is_some() && r.detail.is_empty() {
            r.detail = self.note.clone();
        }
        r
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// One poll of the attached game.
fn walk(source: &dyn Source, g: &mut Game) -> Walk {
    let alive = source.cmdline(g.pid).is_ok_and(|c| contains(&c, EXE.as_bytes())); // exited, reaped or pid reused
    if !alive {
        return Walk::Failed(libc::ESRCH);
    }
    // The build and the cached class pointers belong to the image identified when the game was found. One read
    // per poll re-checks that image's PE TimeDateStamp and SizeOfImage, so that an exec into another image under
    // the same pid (at the same addresses) is never read with the old build's offsets.
    let same = match g.reader.read(g.base, g.span) {
        Ok(head) => pe_identity(&head) == Some(g.ident),
        Err(code) if code == libc::EPERM || code == libc::EACCES || code == libc::ESRCH => return Walk::Failed(code),
        Err(_) => false, // the header is gone: not that image
    };
    if !same {
        return Walk::Replaced;
    }
    match route(&mut *g.reader, g.base, g.build, &mut g.slots) {
        Ok(r) => Walk::Done(r),
        Err(Fail::Soft(status)) => Walk::Done(Reading::new(status)),
        Err(Fail::Os(code)) => Walk::Failed(code),
    }
}

/// The route of one poll over the attached image (`slots` are its class pointers).
struct Route<'a> {
    r: &'a mut dyn Memory,
    b: &'a Build,
    slots: [u64; 8],
}

impl Route<'_> {
    fn read(&mut self, addr: u64, size: usize) -> Result<Vec<u8>, Fail> {
        self.r.read(addr, size).map_err(Fail::Os)
    }

    /// An 8-byte pointer that must look like an object pointer (aligned, user space) or be null.
    fn ptr(&mut self, addr: u64) -> Result<u64, Fail> {
        let value = u64_at(&self.read(addr, 8)?, 0);
        if !value.is_multiple_of(8) || value >= 1 << 47 {
            return Err(Fail::Soft("no data")); // not an object pointer
        }
        Ok(value)
    }

    /// A pointer that must not be null ("not in world" when it is).
    fn obj(&mut self, addr: u64) -> Result<u64, Fail> {
        match self.ptr(addr)? {
            0 => Err(Fail::Soft("not in world")),
            v => Ok(v),
        }
    }

    /// Class slots (bit mask) hit by ClassPrivate and up to MAX_DEPTH SuperStruct steps.
    fn chain(&mut self, o: u64, wanted: u8, done: impl Fn(u8) -> bool) -> Result<u8, Fail> {
        let mut hits = 0u8;
        let mut cls = self.ptr(at(o, self.b.class_private))?;
        for depth in 0..=MAX_DEPTH {
            if cls == 0 {
                break;
            }
            for (k, slot) in self.slots.iter().enumerate() {
                if wanted & (1 << k) != 0 && *slot == cls {
                    hits |= 1 << k;
                }
            }
            if done(hits) || depth == MAX_DEPTH {
                break;
            }
            cls = self.ptr(at(cls, self.b.super_struct))?;
        }
        Ok(hits)
    }

    /// Type check: the object's class is the slot's class or a subclass within MAX_DEPTH steps.
    fn witness(&mut self, o: u64, key: usize) -> Result<(), Fail> {
        if self.chain(o, 1 << key, |h| h != 0)? & (1 << key) == 0 {
            return Err(Fail::Soft("no data"));
        }
        Ok(())
    }
}

fn at(a: u64, off: u64) -> u64 {
    a.saturating_add(off)
}

fn route(r: &mut dyn Memory, base: u64, b: &Build, cached: &mut Option<[u64; 8]>) -> Result<Reading, Fail> {
    let mut w = Route { r, b, slots: [0; 8] };
    match cached {
        Some(slots) => w.slots = *slots,
        None => {
            // UClass* slots are written once at startup; read until all are set
            let mut values = [0u64; 8];
            for (v, rva) in values.iter_mut().zip(b.class_slots()) {
                *v = w.ptr(at(base, rva))?;
            }
            if values.contains(&0) {
                return Err(Fail::Soft("waiting for game"));
            }
            *cached = Some(values);
            w.slots = values;
        }
    }
    let world = w.obj(at(base, b.gworld))?;
    if w.ptr(at(world, b.class_private))? != w.slots[WORLD] {
        return Err(Fail::Soft("no data")); // UWorld: exact class
    }
    let gi = w.obj(at(world, b.world_game_instance))?;
    w.witness(gi, GAME_INSTANCE)?; // type check: UGameInstance
    let arr = w.read(at(gi, b.local_players), 12)?; // LocalPlayers: Data + Num, one read
    let data = u64_at(&arr, b.array_data);
    let num = u32_at(&arr, b.array_num) as i32;
    if num < 1 || data == 0 {
        return Err(Fail::Soft("not in world")); // Num checked before Data[0]
    }
    if !data.is_multiple_of(8) {
        return Err(Fail::Soft("no data"));
    }
    let lp = w.obj(data)?; // element 0 (stride 8)
    w.witness(lp, LOCAL_PLAYER)?; // type check: ULocalPlayer
    let pc = w.obj(at(lp, b.player_controller))?;
    w.witness(pc, PLAYER_CONTROLLER)?; // type check: APlayerController
    // The possessed Pawn alone decides whose threat is shown: the vehicle's when it passes the ADuneVehicle check,
    // the character's when it passes both character checks and is also the AcknowledgedPawn. Anything else shows
    // '-'; there is no fallback to another object.
    let pawn = w.obj(at(pc, b.pawn))?; // AController.Pawn
    let hits =
        w.chain(pawn, 1 << DUNE_VEHICLE | CHARACTER, |h| h & (1 << DUNE_VEHICLE) != 0 || h & CHARACTER == CHARACTER)?;
    let (field, mode) = if hits & (1 << DUNE_VEHICLE) != 0 {
        (b.vehicle_threat, "vehicle") // type check: ADuneVehicle
    } else if hits & CHARACTER == CHARACTER && w.ptr(at(pc, b.acknowledged_pawn))? == pawn {
        (b.character_threat, "on foot") // both character checks, same object
    } else {
        return Err(Fail::Soft("no data"));
    };
    let comp = w.obj(at(pawn, field))?;
    // Type check: USandwormThreatSourceComponent or a subclass within 4 SuperStruct steps. One read when the class
    // is the native one, and the two value offsets hold for any subclass.
    w.witness(comp, THREAT_COMPONENT)?;
    let gap = (b.threat_threshold - b.threat_blob) as usize;
    let raw = w.read(at(comp, b.threat_blob), gap + 4)?; // both f32 leaves, one read
    let blob = f32::from_le_bytes(raw[0..4].try_into().unwrap());
    let threshold = f32::from_le_bytes(raw[gap..gap + 4].try_into().unwrap());
    Ok(reading_for(mode, f64::from(blob), f64::from(threshold)))
}

#[cfg(test)]
pub(crate) mod tests {
    //! The route against an in-memory fake of the game's object graph (the same layout as the fake game
    //! process used by the integration tests), so each rule is checked without any other process.
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::io;
    use std::rc::Rc;

    pub const TS: u32 = 1792504044;
    pub const SIZE: u32 = 220008448;
    const BASE: u64 = 0x1_4000_0000;
    const HEAP: u64 = 0x2_0000_0000;
    const NAMES: [&str; 27] = [
        "c_world",
        "c_gi",
        "c_bp_gi",
        "c_lp",
        "c_pc",
        "c_dune_pc",
        "c_bp_pc",
        "c_char",
        "c_pchar",
        "c_bp_char",
        "c_veh",
        "c_wheeled",
        "c_bp_bike",
        "c_comp",
        "c_bogus",
        "world",
        "gi",
        "lp_array",
        "lp",
        "pc",
        "char",
        "char_comp",
        "veh",
        "veh_comp",
        "bogus",
        "char2",
        "char2_comp",
    ];

    /// Sparse little-endian memory shared between the test and the fake reader.
    #[derive(Default)]
    pub struct Mem {
        bytes: HashMap<u64, u8>,
        pub fail_at: Option<(u64, i32)>, // after this many reads, every read fails with this errno
        pub reads: u64,
    }

    impl Mem {
        fn put(&mut self, addr: u64, data: &[u8]) {
            for (i, b) in data.iter().enumerate() {
                self.bytes.insert(addr + i as u64, *b);
            }
        }
    }

    pub struct FakeReader(pub Rc<RefCell<Mem>>, u64);

    impl Memory for FakeReader {
        fn read(&mut self, addr: u64, size: usize) -> Result<Vec<u8>, i32> {
            if !procfs::user_range(addr, size) {
                return Err(libc::EFAULT);
            }
            self.1 += 1;
            let mut m = self.0.borrow_mut();
            m.reads += 1;
            if let Some((after, code)) = m.fail_at
                && m.reads > after
            {
                return Err(code);
            }
            (0..size as u64).map(|i| m.bytes.get(&(addr + i)).copied().ok_or(libc::EFAULT)).collect()
        }
        fn count(&self) -> u64 {
            self.1
        }
    }

    /// One fake process (pid 4242, our uid), listed with a maps line for the exe at BASE.
    pub struct FakeSource {
        pub state: Rc<RefCell<SourceState>>,
    }
    pub struct SourceState {
        pub alive: bool,
        pub scope: Option<i64>,
        pub ppid: i32,
        pub pids_calls: u32,
        pub statuses: Vec<i32>,
    }

    impl Source for FakeSource {
        fn pids(&self) -> Vec<i32> {
            let mut s = self.state.borrow_mut();
            s.pids_calls += 1;
            if s.alive { vec![4242] } else { vec![] }
        }
        fn uid(&self, _pid: i32) -> io::Result<u32> {
            Ok(procfs::me().1)
        }
        fn cmdline(&self, _pid: i32) -> io::Result<Vec<u8>> {
            if self.state.borrow().alive {
                Ok(b"Z:\\DuneSandbox-Win64-Shipping.exe\0".to_vec())
            } else {
                Err(io::Error::from_raw_os_error(libc::ENOENT))
            }
        }
        fn maps(&self, _pid: i32) -> io::Result<String> {
            Ok(format!(
                "{BASE:x}-{:x} r--p 00000000 103:02 1234 /games/DUNE Awakening/DuneSandbox-Win64-Shipping.exe\n",
                BASE + 0x1000
            ))
        }
        fn ppid(&self, pid: i32) -> io::Result<i32> {
            let mut s = self.state.borrow_mut();
            s.statuses.push(pid);
            Ok(s.ppid)
        }
        fn ptrace_scope(&self) -> Option<i64> {
            self.state.borrow().scope
        }
    }

    /// The fake game's object graph in memory, with commands like the fake game process understands.
    pub struct World {
        pub mem: Rc<RefCell<Mem>>,
        pub state: Rc<RefCell<SourceState>>,
        obj: HashMap<&'static str, u64>,
    }

    impl World {
        pub fn new(timestamp: u32) -> Self {
            let obj: HashMap<&'static str, u64> =
                NAMES.iter().enumerate().map(|(i, n)| (*n, HEAP + 0x1000 * i as u64)).collect();
            let mut m = Mem::default();
            let mut head = vec![0u8; 0x400];
            head[0..2].copy_from_slice(b"MZ");
            head[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
            head[0x80..0x84].copy_from_slice(b"PE\0\0");
            head[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
            head[0x88..0x8C].copy_from_slice(&timestamp.to_le_bytes());
            head[0x98..0x9A].copy_from_slice(&0x20Bu16.to_le_bytes());
            head[0xD0..0xD4].copy_from_slice(&SIZE.to_le_bytes());
            m.put(BASE, &head);
            let p = |m: &mut Mem, a: u64, v: u64| m.put(a, &v.to_le_bytes());
            // class slot RVAs typed in again here (UWorld, UGameInstance, ULocalPlayer, APlayerController,
            // ADuneCharacter, ADunePlayerCharacter, ADuneVehicle, USandwormThreatSourceComponent)
            for (rva, cls) in [
                (0xC4CEF00, "c_world"),
                (0xC49DBB0, "c_gi"),
                (0xC4AD730, "c_lp"),
                (0xC4BB0E8, "c_pc"),
                (0xC166430, "c_char"),
                (0xC16F290, "c_pchar"),
                (0xC1E3D68, "c_veh"),
                (0xC1D8BD8, "c_comp"),
            ] {
                p(&mut m, BASE + rva, obj[cls]);
            }
            for name in NAMES.iter().filter(|n| n.starts_with("c_")) {
                m.put(obj[name], &[0u8; 0x48]); // class objects exist (SuperStruct 0 unless set below)
            }
            for (child, parent) in [
                ("c_bp_gi", "c_gi"),
                ("c_dune_pc", "c_pc"),
                ("c_bp_pc", "c_dune_pc"),
                ("c_pchar", "c_char"),
                ("c_bp_char", "c_pchar"),
                ("c_wheeled", "c_veh"),
                ("c_bp_bike", "c_wheeled"),
            ] {
                p(&mut m, obj[child] + 0x40, obj[parent]);
            }
            for name in [
                "world",
                "gi",
                "lp_array",
                "lp",
                "pc",
                "char",
                "char_comp",
                "veh",
                "veh_comp",
                "bogus",
                "char2",
                "char2_comp",
            ] {
                m.put(obj[name], &[0u8; 0x1000]);
            }
            for (o, cls) in [
                ("world", "c_world"),
                ("gi", "c_bp_gi"),
                ("lp", "c_lp"),
                ("pc", "c_bp_pc"),
                ("char", "c_bp_char"),
                ("char_comp", "c_comp"),
                ("veh", "c_bp_bike"),
                ("veh_comp", "c_comp"),
                ("bogus", "c_bogus"),
                ("char2", "c_bp_char"),
                ("char2_comp", "c_comp"),
            ] {
                p(&mut m, obj[o] + 0x10, obj[cls]);
            }
            p(&mut m, BASE + 0xC4CF528, obj["world"]);
            p(&mut m, obj["world"] + 0x3C8, obj["gi"]);
            p(&mut m, obj["gi"] + 0x38, obj["lp_array"]);
            m.put(obj["gi"] + 0x40, &1i32.to_le_bytes());
            m.put(obj["gi"] + 0x44, &1i32.to_le_bytes());
            p(&mut m, obj["lp_array"], obj["lp"]);
            p(&mut m, obj["lp"] + 0x30, obj["pc"]);
            p(&mut m, obj["pc"] + 0x3F0, obj["char"]);
            p(&mut m, obj["pc"] + 0x388, obj["char"]);
            p(&mut m, obj["char"] + 0xE88, obj["char_comp"]);
            p(&mut m, obj["veh"] + 0xE08, obj["veh_comp"]);
            p(&mut m, obj["char2"] + 0xE88, obj["char2_comp"]);
            for comp in ["char_comp", "veh_comp", "char2_comp"] {
                for off in (0x300..0x380).step_by(4) {
                    m.put(obj[comp] + off, &5000f32.to_le_bytes());
                }
            }
            for (comp, blob) in [("char_comp", 42.5f32), ("veh_comp", 80.0), ("char2_comp", 7.0)] {
                m.put(obj[comp] + 0x334, &blob.to_le_bytes());
                m.put(obj[comp] + 0x348, &100f32.to_le_bytes());
            }
            let state = Rc::new(RefCell::new(SourceState {
                alive: true,
                scope: Some(0),
                ppid: 0,
                pids_calls: 0,
                statuses: vec![],
            }));
            World { mem: Rc::new(RefCell::new(m)), state, obj }
        }

        /// `f obj off value`, `i obj off value`, `p obj off target|0` like the fake game process.
        pub fn cmd(&self, line: &str) {
            let w: Vec<&str> = line.split_whitespace().collect();
            let at = self.obj[w[1]] + u64::from_str_radix(w[2].trim_start_matches("0x"), 16).unwrap();
            let mut m = self.mem.borrow_mut();
            match w[0] {
                "f" => m.put(at, &w[3].parse::<f32>().unwrap().to_le_bytes()),
                "i" => m.put(at, &w[3].parse::<i32>().unwrap().to_le_bytes()),
                "p" => m.put(at, &(if w[3] == "0" { 0 } else { self.obj[w[3]] }).to_le_bytes()),
                _ => panic!("{line}"),
            }
        }

        pub fn set_timestamp(&self, ts: u32) {
            self.mem.borrow_mut().put(BASE + 0x88, &ts.to_le_bytes());
        }

        pub fn meter(&self, launched: bool) -> Meter {
            let mem = self.mem.clone();
            Meter::new(
                Box::new(FakeSource { state: self.state.clone() }),
                Box::new(move |_pid| -> Box<dyn Memory> { Box::new(FakeReader(mem.clone(), 0)) }),
                launched,
                Duration::ZERO,
            )
        }
    }

    fn dash(w: &World, m: &mut Meter, cmds: &[&str]) -> Reading {
        for c in cmds {
            w.cmd(c);
        }
        let r = m.poll();
        assert_eq!((r.percent, r.fraction), (None, None), "{cmds:?}");
        assert!(crate::text::format_line(&r).contains(" - "), "{cmds:?}");
        r
    }

    fn setup() -> (World, Meter) {
        let w = World::new(TS);
        let m = w.meter(false);
        (w, m)
    }

    #[test]
    fn builds_table_identity() {
        let keys: Vec<(u32, u32)> = BUILDS.iter().map(|b| (b.pe_timestamp, b.size_of_image)).collect();
        assert_eq!(keys, vec![(1792504044, 220008448)]);
        assert_eq!(BUILDS[0].build, "26083551");
    }

    #[test]
    fn on_foot_and_read_counts() {
        let (_w, mut m) = setup();
        let r = m.poll();
        assert_eq!((r.status.as_str(), r.percent), ("on foot", Some(42)));
        assert!((r.fraction.unwrap() - 0.425).abs() < 1e-9);
        assert_eq!(m.attached_build(), Some("26083551"));
        assert_eq!(m.reads, 1 + 8 + 21); // header + 8 class slots + header start and the walk
        assert_eq!(m.poll().percent, Some(42));
        assert_eq!(m.reads, 21);
    }

    #[test]
    fn vehicle_and_back() {
        let (w, mut m) = setup();
        w.cmd("p pc 0x388 veh");
        m.poll();
        let r = m.poll();
        assert_eq!((r.status.as_str(), r.percent), ("vehicle", Some(80)));
        assert_eq!(m.reads, 20);
        w.cmd("p pc 0x388 char");
        assert_eq!(m.poll().percent, Some(42));
    }

    #[test]
    fn vehicle_component_wins_over_character() {
        let (w, mut m) = setup();
        w.cmd("p pc 0x388 veh");
        w.cmd("f char_comp 0x334 10");
        assert_eq!(m.poll().percent, Some(80));
    }

    #[test]
    fn the_possessed_character_is_read() {
        let (w, mut m) = setup();
        w.cmd("p pc 0x388 char2");
        w.cmd("p pc 0x3f0 char2");
        let r = m.poll();
        assert_eq!((r.status.as_str(), r.percent), ("on foot", Some(7)));
    }

    // Owner selection: the possessed Pawn decides; anything else shows '-', never another object's value.
    #[test]
    fn owner_selection_never_falls_back() {
        let cases: &[(&[&str], &str)] = &[
            (&["p pc 0x388 0"], "not in world"),
            (&["p pc 0x388 veh", "p veh 0x10 c_bogus"], "no data"),
            (
                &[
                    "p c_wheeled 0x40 c_bogus",
                    "p c_bogus 0x40 c_world",
                    "p c_world 0x40 c_lp",
                    "p c_lp 0x40 c_veh",
                    "p pc 0x388 veh",
                ],
                "no data",
            ),
            (&["p pc 0x3f0 char2"], "no data"),
            (&["p pc 0x388 lp"], "no data"),
            (&["p pc 0x388 bogus"], "no data"),
        ];
        for (cmds, status) in cases {
            let (w, mut m) = setup();
            assert_eq!(m.poll().percent, Some(42));
            assert_eq!(dash(&w, &mut m, cmds).status, *status, "{cmds:?}");
        }
    }

    #[test]
    fn misaligned_pointer_is_rejected() {
        let (w, mut m) = setup();
        w.cmd("i pc 0x388 12345");
        let r = m.poll();
        assert_eq!((r.status.as_str(), r.percent), ("no data", None));
    }

    #[test]
    fn local_players_num_checked_before_data() {
        let (w, mut m) = setup();
        m.poll();
        w.cmd("i gi 0x40 0");
        let r = m.poll();
        assert_eq!((r.status.as_str(), r.percent), ("not in world", None));
        assert_eq!(m.reads, 7); // stops at the TArray: Data[0] is never read
        w.cmd("i gi 0x40 -1");
        assert_eq!(m.poll().percent, None);
        w.cmd("i gi 0x40 1");
        w.cmd("p gi 0x38 0");
        assert_eq!(m.poll().status, "not in world");
    }

    #[test]
    fn missing_objects() {
        let (w, mut m) = setup();
        w.cmd("p pc 0x388 0");
        w.cmd("p pc 0x3f0 0");
        assert_eq!(m.poll().status, "not in world");
        let (w, mut m) = setup();
        w.cmd("p char 0xe88 0");
        assert_eq!(m.poll().percent, None);
    }

    #[test]
    fn every_type_check() {
        let cases: &[&[&str]] = &[
            &["p char 0xe88 bogus"],
            &["p world 0x10 c_bogus"],
            &["p gi 0x10 c_bogus"],
            &["p lp 0x10 c_bogus"],
            &["p pc 0x10 c_lp"],
            &["p pc 0x388 lp", "p pc 0x3f0 lp"],
            &["p char 0x10 c_char"],
            &["p pc 0x388 veh", "p veh 0xe08 bogus"],
        ];
        for cmds in cases {
            let (w, mut m) = setup();
            assert_eq!(dash(&w, &mut m, cmds).status, "no data", "{cmds:?}");
        }
    }

    #[test]
    fn chain_depth_is_four() {
        let (w, mut m) = setup();
        for c in ["p c_wheeled 0x40 c_bogus", "p c_bogus 0x40 c_world", "p c_world 0x40 c_veh", "p pc 0x388 veh"] {
            w.cmd(c);
        }
        assert_eq!(m.poll().percent, Some(80));
        dash(&w, &mut m, &["p c_world 0x40 c_lp", "p c_lp 0x40 c_veh"]);
    }

    #[test]
    fn values_outside_the_domain_show_dash() {
        let (w, mut m) = setup();
        for (blob, thr, why) in [
            ("NaN", "100", "no value"),
            ("-1", "100", "no value"),
            ("inf", "100", "no value"),
            ("2e6", "100", "no value"),
            ("42", "0", "no threshold"),
            ("42", "-5", "no threshold"),
            ("42", "NaN", "no threshold"),
            ("42", "inf", "no threshold"),
        ] {
            w.cmd(&format!("f char_comp 0x334 {blob}"));
            w.cmd(&format!("f char_comp 0x348 {thr}"));
            let r = m.poll();
            assert_eq!((r.percent, r.fraction), (None, None), "{blob} {thr}");
            assert_eq!(r.status, format!("on foot, {why}"));
            assert!(!crate::text::format_line(&r).contains("0%"));
        }
    }

    #[test]
    fn genuine_zero_floor_and_clamp() {
        let (w, mut m) = setup();
        w.cmd("f char_comp 0x334 0");
        let r = m.poll();
        assert_eq!((r.percent, r.fraction), (Some(0), Some(0.0)));
        for (blob, want, frac) in [
            ("29", 29, 0.29),
            ("99.99", 99, 0.9999),
            ("100", 100, 1.0),
            ("150", 100, 1.0),
            ("1000", 100, 1.0),
            ("0.5", 0, 0.005),
        ] {
            w.cmd(&format!("f char_comp 0x334 {blob}"));
            let r = m.poll();
            assert_eq!(r.percent, Some(want), "{blob}");
            assert!((r.fraction.unwrap() - frac).abs() < 1e-5, "{blob}");
        }
    }

    #[test]
    fn reading_rules() {
        assert_eq!(reading_for("on foot", 42.5, 100.0).percent, Some(42));
        assert_eq!(reading_for("vehicle", 250.0, 100.0), Reading::value("vehicle", 100, 1.0));
        assert_eq!(reading_for("on foot", 0.0, 100.0), Reading::value("on foot", 0, 0.0));
        assert_eq!(reading_for("on foot", -0.001, 100.0).status, "on foot, no value");
        assert_eq!(reading_for("on foot", 1e6, 1e6).percent, Some(100));
        assert_eq!(reading_for("on foot", 1.0, 1e6 + 1.0).status, "on foot, no threshold");
        assert_eq!(reading_for("on foot", 1.0, f64::MIN_POSITIVE).percent, Some(100));
    }

    #[test]
    fn unknown_build_is_unsupported_never_guessed() {
        let w = World::new(1700000000);
        let mut m = w.meter(false);
        let r = m.poll();
        assert_eq!((r.status.as_str(), r.percent), ("Unsupported game version", None));
        assert!(r.hint.contains("1700000000") && r.detail.contains("1700000000") && r.detail.contains("26083551"));
        assert!(r.detail.contains("(2023-11-14 22:13 UTC)"), "{}", r.detail);
        assert_eq!(m.reads, 1); // only the PE header was read
        assert_eq!(m.attached_build(), None);
        let line = crate::text::format_line(&r);
        assert!(line.contains("Unsupported game version") && line.contains("1700000000"));
    }

    #[test]
    fn image_replaced_is_identified_again() {
        let (w, mut m) = setup();
        assert_eq!(m.poll().percent, Some(42));
        w.set_timestamp(1700000000); // the same process now runs another image at the same addresses
        let r = m.poll();
        assert_eq!((r.status.as_str(), r.percent, r.fraction), ("Unsupported game version", None, None));
        assert!(r.detail.contains("1700000000"));
        assert_eq!(m.attached_build(), None);
        assert_eq!(m.poll().status, "Unsupported game version");
        w.set_timestamp(TS);
        w.cmd("f char_comp 0x334 61");
        assert_eq!(m.poll().percent, Some(61));
    }

    #[test]
    fn image_is_checked_with_one_read_per_poll() {
        let (w, mut m) = setup();
        m.poll();
        let before = w.mem.borrow().reads;
        m.poll();
        assert_eq!(m.reads, 21);
        assert_eq!(w.mem.borrow().reads - before, 21);
    }

    #[test]
    fn game_exit_between_polls_and_mid_poll() {
        let (w, mut m) = setup();
        assert_eq!(m.poll().percent, Some(42));
        w.state.borrow_mut().alive = false;
        let r = m.poll();
        assert_eq!((r.status.as_str(), r.percent), ("waiting for game", None));
        assert_eq!(m.attached_build(), None);
        assert_eq!(m.poll().status, "waiting for game");
        // mid-poll: the 12th read of the process finds it gone (ESRCH)
        let (w, mut m) = setup();
        w.mem.borrow_mut().fail_at = Some((12, libc::ESRCH));
        let r = m.poll();
        assert_eq!((r.status.as_str(), r.percent), ("waiting for game", None));
        assert_eq!(m.attached_build(), None);
    }

    #[test]
    fn stale_pointer_is_no_data_and_stays_attached() {
        let (w, mut m) = setup();
        m.poll();
        let after = w.mem.borrow().reads + 5;
        w.mem.borrow_mut().fail_at = Some((after, libc::EFAULT));
        assert_eq!(m.poll().status, "no data");
        assert_eq!(m.attached_build(), Some("26083551"));
    }

    #[test]
    fn rescan_only_every_scan_interval_while_not_attached() {
        let w = World::new(TS);
        w.state.borrow_mut().alive = false;
        let mem = w.mem.clone();
        let mut m = Meter::new(
            Box::new(FakeSource { state: w.state.clone() }),
            Box::new(move |_| -> Box<dyn Memory> { Box::new(FakeReader(mem.clone(), 0)) }),
            false,
            Duration::from_secs(2),
        );
        assert_eq!(m.poll().status, "waiting for game");
        w.state.borrow_mut().alive = true;
        assert_eq!(m.poll().status, "waiting for game"); // within 2 s: no new scan
        assert_eq!(w.state.borrow().pids_calls, 1);
    }

    fn denied(w: &World, launched: bool) -> Meter {
        Meter::new(
            Box::new(FakeSource { state: w.state.clone() }),
            Box::new(|_| -> Box<dyn Memory> {
                struct Denied(u64);
                impl Memory for Denied {
                    fn read(&mut self, _: u64, _: usize) -> Result<Vec<u8>, i32> {
                        self.0 += 1;
                        Err(libc::EPERM)
                    }
                    fn count(&self) -> u64 {
                        self.0
                    }
                }
                Box::new(Denied(0))
            }),
            launched,
            Duration::ZERO,
        )
    }

    #[test]
    fn permission_refused_per_scope() {
        for (scope, launched, needles) in [
            (Some(0), false, vec!["same user"]),
            (Some(1), false, vec!["--launch %command%", "sudo sysctl kernel.yama.ptrace_scope=0"]),
            (Some(1), true, vec!["sudo sysctl kernel.yama.ptrace_scope=0", "wrapper"]),
            (Some(2), false, vec!["sudo sysctl kernel.yama.ptrace_scope=1"]),
            (Some(3), false, vec!["reboot", "/etc/sysctl.d/"]),
            (None, false, vec!["Yama"]),
        ] {
            let w = World::new(TS);
            w.state.borrow_mut().scope = scope;
            w.state.borrow_mut().ppid = std::process::id() as i32; // a descendant when launched
            let mut m = denied(&w, launched);
            let r = m.poll();
            assert_eq!((r.status.as_str(), r.percent), ("no permission", None));
            for n in needles {
                assert!(r.detail.contains(n), "{scope:?} {launched} {n}");
            }
            assert!(r.detail.contains("ptrace_scope"));
            assert!(!r.hint.is_empty());
        }
    }

    #[test]
    fn launched_reads_a_descendant_and_refuses_others() {
        let w = World::new(TS);
        w.state.borrow_mut().ppid = std::process::id() as i32;
        assert_eq!(w.meter(true).poll().percent, Some(42));
        for scope in [Some(1), Some(2), Some(3)] {
            let w = World::new(TS);
            w.state.borrow_mut().ppid = 1;
            w.state.borrow_mut().scope = scope;
            let mut m = w.meter(true);
            let r = m.poll();
            assert_eq!((r.status.as_str(), r.percent), ("not started by --launch", None));
            assert!(r.detail.contains("4242") && r.detail.contains("Steam started the game outside DeHulud"));
            assert!(!r.detail.contains("%command%"));
            assert_eq!(m.reads, 0); // nothing was read from it
        }
        for scope in [Some(0), None] {
            let w = World::new(TS);
            w.state.borrow_mut().ppid = 1;
            w.state.borrow_mut().scope = scope;
            let mut m = w.meter(true);
            let r = m.poll();
            assert_eq!((r.status.as_str(), r.percent), ("on foot", Some(42)));
            assert!(
                r.detail.contains("found the game outside --launch; reading it anyway") && r.detail.contains("4242")
            );
            assert_eq!(m.poll().percent, Some(42));
        }
        // lowering the scope to 0 lets the running meter read it
        let w = World::new(TS);
        w.state.borrow_mut().ppid = 1;
        w.state.borrow_mut().scope = Some(1);
        let mut m = w.meter(true);
        assert_eq!(m.poll().status, "not started by --launch");
        w.state.borrow_mut().scope = Some(0);
        assert_eq!(m.poll().percent, Some(42));
    }

    /// Made-up pids: no real process is looked at.
    struct Tree {
        foreign: Vec<i32>,
        statuses: RefCell<Vec<i32>>,
        uids: RefCell<Vec<i32>>,
    }
    impl Source for Tree {
        fn pids(&self) -> Vec<i32> {
            vec![]
        }
        fn uid(&self, pid: i32) -> io::Result<u32> {
            self.uids.borrow_mut().push(pid);
            Ok(procfs::me().1 + u32::from(self.foreign.contains(&pid)))
        }
        fn cmdline(&self, _: i32) -> io::Result<Vec<u8>> {
            Ok(vec![])
        }
        fn maps(&self, _: i32) -> io::Result<String> {
            Ok(String::new())
        }
        fn ppid(&self, pid: i32) -> io::Result<i32> {
            self.statuses.borrow_mut().push(pid);
            Ok(match pid {
                900000001 => 900000002,
                900000002 => std::process::id() as i32,
                _ => 1,
            })
        }
        fn ptrace_scope(&self) -> Option<i64> {
            None
        }
    }

    fn tree_meter(foreign: Vec<i32>) -> (Meter, Rc<Tree>) {
        let t = Rc::new(Tree { foreign, statuses: RefCell::new(vec![]), uids: RefCell::new(vec![]) });
        struct Shared(Rc<Tree>);
        impl Source for Shared {
            fn pids(&self) -> Vec<i32> {
                self.0.pids()
            }
            fn uid(&self, p: i32) -> io::Result<u32> {
                self.0.uid(p)
            }
            fn cmdline(&self, p: i32) -> io::Result<Vec<u8>> {
                self.0.cmdline(p)
            }
            fn maps(&self, p: i32) -> io::Result<String> {
                self.0.maps(p)
            }
            fn ppid(&self, p: i32) -> io::Result<i32> {
                self.0.ppid(p)
            }
            fn ptrace_scope(&self) -> Option<i64> {
                None
            }
        }
        let m = Meter::new(
            Box::new(Shared(t.clone())),
            Box::new(|p| -> Box<dyn Memory> { Box::new(procfs::Reader::new(p)) }),
            true,
            Duration::ZERO,
        );
        (m, t)
    }

    #[test]
    fn ancestry_walk_stops_before_another_users_process() {
        let (m, t) = tree_meter(vec![900000002]);
        assert!(!m.descends(900000001));
        assert_eq!(*t.statuses.borrow(), vec![900000001]); // the other user's process was never read
        assert!(t.uids.borrow().contains(&900000002));
        let (m, t) = tree_meter(vec![]);
        assert!(m.descends(900000001));
        assert_eq!(*t.statuses.borrow(), vec![900000001, 900000002]);
    }

    #[test]
    fn image_base_parsing() {
        let maps = "100000-200000 r--p 00000000 08:01 5 /x/DuneSandbox-Win64-Shipping.exe.bak\n\
                    140001000-140100000 r-xp 00001000 08:01 9 /g/DUNE Awakening/DuneSandbox-Win64-Shipping.exe\n\
                    140000000-140001000 r--p 00000000 08:01 9 /g/DUNE Awakening/DuneSandbox-Win64-Shipping.exe\n\
                    7f0000000000-7f0000001000 r--p 00000000 08:01 9 /g/x/DuneSandbox-Win64-Shipping.exe (deleted)\n\
                    garbage line\n";
        assert_eq!(image_base(maps), Some((0x140000000, "/g/DUNE Awakening/DuneSandbox-Win64-Shipping.exe".into())));
        assert_eq!(image_base("140000000-140001000 r--p 00001000 08:01 9 /g/DuneSandbox-Win64-Shipping.exe\n"), None);
        assert_eq!(
            image_base("7f00-7f01 r--p 00000000 08:01 9          /g/x/DuneSandbox-Win64-Shipping.exe (deleted)\n"),
            Some((0x7f00, "/g/x/DuneSandbox-Win64-Shipping.exe".into()))
        );
    }

    #[test]
    fn pe_identity_parsing() {
        let mut head = vec![0u8; 0x400];
        head[0..2].copy_from_slice(b"MZ");
        head[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        head[0x80..0x84].copy_from_slice(b"PE\0\0");
        head[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
        head[0x88..0x8C].copy_from_slice(&1792504044u32.to_le_bytes());
        head[0x98..0x9A].copy_from_slice(&0x20Bu16.to_le_bytes());
        head[0xD0..0xD4].copy_from_slice(&220008448u32.to_le_bytes());
        assert_eq!(pe_identity(&head), Some((1792504044, 220008448)));
        assert_eq!(pe_identity(&head[..0xD8]), Some((1792504044, 220008448))); // the per-poll span
        assert_eq!(pe_identity(&head[..0xD7]), None);
        for (at, bytes) in
            [(0, &b"ZM"[..]), (0x80, b"PX"), (0x84, b"\x4c\x01"), (0x98, b"\x0b\x01"), (0x3C, b"\xf0\x03")]
        {
            let mut bad = head.clone();
            bad[at..at + bytes.len()].copy_from_slice(bytes);
            assert_eq!(pe_identity(&bad), None, "{at:#x}");
        }
        assert_eq!(pe_identity(b"MZ"), None);
    }
}
