//! Character vitals, weak target resolution and net drops between adjacent polls.

use crate::meter::{Attached, Damage};
use crate::sections::{Feature, Features};
use std::collections::HashMap;
use std::time::Instant;

type Key = (i32, i32);
type Sample = (f64, f64, f64);
type Baseline = Option<(f64, Option<f64>, Instant)>;
type Channels = (Baseline, Baseline);

/// Optional damage route for one supported game image.
#[derive(Debug)]
pub struct Layout {
    pub attribute_set: u64,
    pub health: u64,
    pub maximum: u64,
    pub power: u64,
    pub payload: u64,
    pub current: u64,
    pub cls_attributes: u64,
    pub cls_controller: u64,
    pub coordinator: u64,
    pub owner: u64,
    pub cls_coordinator: u64,
    pub aim: u64,
    pub soft_lock: u64,
    pub weak_index: usize,
    pub weak_serial: usize,
    pub chunks: u64,
    pub count: u64,
    pub shift: u32,
    pub stride: usize,
    pub item_object: usize,
    pub item_flags: usize,
    pub item_serial: usize,
    pub reject_flags: u32,
}

pub static BUILD_26333948: Layout = Layout {
    attribute_set: 0x1400,      // ADuneCharacter.m_DuneCharacterAttributeSet
    health: 0x98,               // UDuneCharacterAttributeSet.CurrentHealth
    maximum: 0xC0,              // UDuneCharacterAttributeSet.MaxHealth
    power: 0x320,               // UDuneCharacterAttributeSet.CurrentPower
    payload: 0x18,              // FglGameplayAttributeData.CurrentValuePtr
    current: 0xC,               // FGameplayAttributeData.CurrentValue
    cls_attributes: 0xC146A88,  // UDuneCharacterAttributeSet class slot RVA
    cls_controller: 0xC175948,  // ADunePlayerController class slot RVA
    coordinator: 0x1048,        // ADunePlayerController.m_InventoryCoordinator
    owner: 0x268,               // UInventoryCoordinator.m_OwnerCharacter
    cls_coordinator: 0xC1CD1D8, // UInventoryCoordinator class slot RVA
    aim: 0x1E58,                // ADunePlayerCharacter.m_CachedAimTarget
    soft_lock: 0x1D80,          // ADunePlayerCharacter.m_SoftLockTarget
    weak_index: 0,              // FWeakObjectPtr.ObjectIndex
    weak_serial: 4,             // FWeakObjectPtr.ObjectSerialNumber
    chunks: 0xC337DC0,          // GUObjectArray.ObjObjects.Objects
    count: 0xC337DD4,           // GUObjectArray.ObjObjects.NumElements
    shift: 16,                  // FChunkedFixedUObjectArray chunk index shift
    stride: 24,                 // FUObjectItem stride
    item_object: 0,             // FUObjectItem.Object
    item_flags: 8,              // FUObjectItem.Flags
    item_serial: 16,            // FUObjectItem.SerialNumber
    reject_flags: 0x30200000,   // FUObjectItem.Flags weak reject mask
};

fn channel(old: Baseline, value: f64, maximum: Option<f64>, now: Instant) -> (Baseline, Option<f64>) {
    if !value.is_finite() || value < 0.0 || maximum.is_some_and(|m| !m.is_finite() || m <= 0.0 || value > m) {
        return (None, None);
    }
    let drop = old.and_then(|(v, m, at)| {
        (now.checked_duration_since(at).is_some_and(|d| d.as_secs_f64() < 0.5) && m == maximum && v > 0.0 && value < v)
            .then_some(v - value)
    });
    (if value > 0.0 { Some((value, maximum, now)) } else { None }, drop)
}

/// Histories are keyed by object index and serial, independent of targeting role.
#[derive(Default)]
pub struct History {
    own: Baseline,
    targets: HashMap<Key, Channels>,
}

impl History {
    pub fn reset(&mut self, f: Feature) {
        match f {
            Feature::Worm => {}
            Feature::Taken => self.own = None,
            Feature::Dealt => self.targets.clear(),
        }
    }

    pub fn taken(&mut self, sample: Option<(f64, f64)>, now: Instant) -> Option<Option<f64>> {
        let Some((health, maximum)) = sample else {
            self.own = None;
            return None;
        };
        let (seed, drop) = channel(self.own, health, Some(maximum), now);
        self.own = seed;
        if !health.is_finite() || !maximum.is_finite() || maximum <= 0.0 || !(0.0..=maximum).contains(&health) {
            None
        } else {
            Some(drop)
        }
    }

    pub fn dealt(&mut self, samples: &[(Key, Option<Sample>)], now: Instant) -> (Option<f64>, Option<f64>) {
        self.targets.retain(|k, _| samples.iter().any(|(key, _)| key == k));
        let (mut health, mut shield) = (0.0, 0.0);
        for (key, sample) in samples {
            let Some((h, m, s)) = sample else {
                self.targets.remove(key);
                continue;
            };
            let old = self.targets.get(key).copied().unwrap_or_default();
            let (hs, hd) = channel(old.0, *h, Some(*m), now);
            let (ss, sd) = channel(old.1, *s, None, now);
            self.targets.insert(*key, (hs, ss));
            health += hd.unwrap_or(0.0);
            shield += sd.unwrap_or(0.0);
        }
        ((health > 0.0).then_some(health), (shield > 0.0).then_some(shield))
    }
}

fn read(g: &mut Attached, address: u64, size: usize) -> Result<Vec<u8>, ()> {
    g.mem.read(address, size).map_err(|_| ())
}

fn ptr(g: &mut Attached, address: u64) -> Result<u64, ()> {
    aligned(g, address, 8)
}

fn aligned(g: &mut Attached, address: u64, alignment: u64) -> Result<u64, ()> {
    let raw = read(g, address, 8)?;
    let value = u64::from_le_bytes(raw.try_into().map_err(|_| ())?);
    if value % alignment != 0 || value >= 1 << 47 { Err(()) } else { Ok(value) }
}

fn object(g: &mut Attached, address: u64) -> Result<u64, ()> {
    match ptr(g, address)? {
        0 => Err(()),
        p => Ok(p),
    }
}

fn float(g: &mut Attached, address: u64) -> Result<f64, ()> {
    Ok(f32::from_le_bytes(read(g, address, 4)?.try_into().map_err(|_| ())?) as f64)
}

fn class(g: &mut Attached, obj: u64, slot: u64, exact: bool) -> Result<bool, ()> {
    let wanted = object(g, g.base + slot)?;
    let mut cls = ptr(g, obj + g.build.class_private)?;
    for depth in 0..=4 {
        if cls == 0 {
            return Ok(false);
        }
        if cls == wanted {
            return Ok(true);
        }
        if exact || depth == 4 {
            break;
        }
        cls = ptr(g, cls + g.build.super_struct)?;
    }
    Ok(false)
}

fn require(g: &mut Attached, obj: u64, slot: u64, exact: bool) -> Result<(), ()> {
    if class(g, obj, slot, exact)? { Ok(()) } else { Err(()) }
}

fn vitals(g: &mut Attached, obj: u64, shield: bool) -> Result<Sample, ()> {
    let d = g.build.damage.ok_or(())?;
    let attrs = object(g, obj + d.attribute_set)?; // ADuneCharacter.m_DuneCharacterAttributeSet
    require(g, attrs, d.cls_attributes, true)?; // UDuneCharacterAttributeSet class slot RVA
    let payload = aligned(g, attrs + d.health + d.payload, 4)?; // CurrentHealth + CurrentValuePtr
    if payload == 0 {
        return Err(());
    }
    let health = float(g, payload)?;
    let maximum = float(g, attrs + d.maximum + d.current)?; // MaxHealth + CurrentValue
    let power = if shield { float(g, attrs + d.power + d.current)? } else { 0.0 }; // CurrentPower + CurrentValue
    Ok((health, maximum, power))
}

fn handle(g: &mut Attached, address: u64) -> Result<Option<Key>, ()> {
    let d = g.build.damage.ok_or(())?;
    let raw = read(g, address, 8)?;
    let key = (
        i32::from_le_bytes(raw[d.weak_index..d.weak_index + 4].try_into().unwrap()),
        i32::from_le_bytes(raw[d.weak_serial..d.weak_serial + 4].try_into().unwrap()),
    );
    Ok((key.0 >= 0 && key.1 != 0).then_some(key))
}

fn item_fields(raw: &[u8], d: &Layout) -> (u64, u32, i32) {
    (
        u64::from_le_bytes(raw[d.item_object..d.item_object + 8].try_into().unwrap()), // FUObjectItem.Object
        u32::from_le_bytes(raw[d.item_flags..d.item_flags + 4].try_into().unwrap()),   // FUObjectItem.Flags
        i32::from_le_bytes(raw[d.item_serial..d.item_serial + 4].try_into().unwrap()),
    ) // FUObjectItem.SerialNumber
}

fn target(g: &mut Attached, key: Key) -> Result<Sample, ()> {
    let d = g.build.damage.ok_or(())?;
    let count = i32::from_le_bytes(read(g, g.base + d.count, 4)?.try_into().unwrap()); // GUObjectArray.ObjObjects.NumElements
    if key.0 < 0 || key.0 >= count {
        return Err(());
    }
    let chunks = object(g, g.base + d.chunks)?; // GUObjectArray.ObjObjects.Objects
    let index = key.0 as u64;
    let chunk = object(g, chunks + (index >> d.shift) * 8)?; // FChunkedFixedUObjectArray chunk index shift
    let item = chunk + (index & ((1 << d.shift) - 1)) * d.stride as u64; // FUObjectItem stride
    let before = item_fields(&read(g, item, d.stride)?, d);
    let (obj, flags, serial) = before;
    if serial != key.1 || flags & d.reject_flags != 0 || obj == 0 || obj % 8 != 0 || obj >= 1 << 47 {
        return Err(());
    }
    require(g, obj, g.build.cls_dune_character, false)?;
    let values = vitals(g, obj, true)?;
    if item_fields(&read(g, item, d.stride)?, d) != before {
        return Err(());
    }
    Ok(values)
}

/// The production damage readers. begin/reset manage history without reading memory.
pub struct DamageReader {
    history: History,
    image: u64,
    context: Option<(u64, u64, u64)>,
    path: Option<(u64, u64, &'static str)>,
    now: Instant,
    recent: Vec<(Key, Instant)>,
}

impl Default for DamageReader {
    fn default() -> Self {
        Self {
            history: History::default(),
            image: 0,
            context: None,
            path: None,
            now: Instant::now(),
            recent: Vec::new(),
        }
    }
}

impl DamageReader {
    fn clear(&mut self) {
        self.history = History::default();
        self.recent.clear();
        self.path = None;
        self.context = None;
    }

    pub fn prepare(&mut self, now: Instant, enabled: Features, image: u64) {
        self.path = None;
        self.now = now;
        if self.image != image {
            self.clear();
            self.image = image;
        }
        if !enabled.taken {
            self.reset_feature(Feature::Taken);
        }
        if !enabled.dealt {
            self.reset_feature(Feature::Dealt);
        }
    }

    pub fn reset_feature(&mut self, f: Feature) {
        self.history.reset(f);
        if f == Feature::Dealt {
            self.recent.clear();
        }
        self.path = None;
    }

    fn character(&mut self, g: &mut Attached) -> Result<(u64, u64, &'static str), ()> {
        if g.build.damage.is_none() {
            return Err(());
        }
        if let Some(path) = self.path {
            return Ok(path);
        }
        let b = g.build;
        let d = b.damage.ok_or(())?;
        let world = object(g, g.base + b.gworld)?;
        require(g, world, b.cls_world, true)?;
        let gi = object(g, world + b.world_game_instance)?;
        require(g, gi, b.cls_game_instance, false)?;
        let arr = read(g, gi + b.local_players, 12)?;
        let data = u64::from_le_bytes(arr[..8].try_into().unwrap());
        let num = i32::from_le_bytes(arr[8..12].try_into().unwrap());
        if num < 1 || data == 0 || data % 8 != 0 || data >= 1 << 47 {
            return Err(());
        }
        let lp = object(g, data)?;
        require(g, lp, b.cls_local_player, false)?;
        let pc = object(g, lp + b.player_controller)?;
        require(g, pc, b.cls_player_controller, false)?;
        let pawn = object(g, pc + b.pawn)?;
        let (own, mode) = if class(g, pawn, b.cls_dune_vehicle, false)? {
            require(g, pc, d.cls_controller, false)?; // ADunePlayerController class slot RVA
            let coordinator = object(g, pc + d.coordinator)?; // ADunePlayerController.m_InventoryCoordinator
            require(g, coordinator, d.cls_coordinator, false)?; // UInventoryCoordinator class slot RVA
            (object(g, coordinator + d.owner)?, "vehicle") // UInventoryCoordinator.m_OwnerCharacter
        } else {
            if object(g, pc + b.acknowledged_pawn)? != pawn {
                return Err(());
            }
            (pawn, "on foot")
        };
        require(g, own, b.cls_dune_character, false)?;
        require(g, own, b.cls_dune_player_character, false)?;
        let context = (self.image, world, own);
        if self.context != Some(context) {
            self.clear();
            self.context = Some(context);
        }
        self.path = Some((world, own, mode));
        Ok((world, own, mode))
    }
}

impl Damage for DamageReader {
    fn begin(&mut self, now: Instant, enabled: Features, image: u64) {
        self.prepare(now, enabled, image);
    }

    fn reset(&mut self, f: Feature) {
        self.reset_feature(f);
    }

    fn taken(&mut self, g: &mut Attached) -> Option<Option<f64>> {
        crate::testhook::route("damage_taken");
        let Ok((_, own, _)) = self.character(g) else {
            self.clear();
            return None;
        };
        let sample = vitals(g, own, false).ok().map(|(h, m, _)| (h, m));
        self.history.taken(sample, self.now)
    }

    fn dealt(&mut self, g: &mut Attached) -> Option<(Option<f64>, Option<f64>)> {
        crate::testhook::route("damage_dealt");
        let Ok((_, own, _)) = self.character(g) else {
            self.clear();
            return None;
        };
        let roles = (|| -> Result<_, ()> {
            let d = g.build.damage.ok_or(())?;
            Ok((
                handle(g, own + d.aim)?, // ADunePlayerCharacter.m_CachedAimTarget
                handle(g, own + d.soft_lock)?,
            )) // ADunePlayerCharacter.m_SoftLockTarget
        })();
        let Ok((aim, soft)) = roles else {
            self.reset_feature(Feature::Dealt);
            return Some((None, None));
        };
        self.recent.retain(|(key, at)| {
            Some(*key) != aim && self.now.checked_duration_since(*at).is_some_and(|d| d.as_secs() < 18)
        });
        if let Some(key) = aim {
            self.recent.insert(0, (key, self.now));
        }
        self.recent.truncate(5);
        let mut keys = Vec::new();
        for key in [aim, soft].into_iter().flatten() {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        let recent: Vec<Key> = self.recent.iter().map(|(k, _)| *k).filter(|k| !keys.contains(k)).take(4).collect();
        keys.extend(recent);
        let samples: Vec<_> = keys.into_iter().take(6).map(|key| (key, target(g, key).ok())).collect();
        Some(self.history.dealt(&samples, self.now))
    }

    fn mode(&mut self, g: &mut Attached) -> Option<String> {
        match self.character(g) {
            Ok((_, _, mode)) => Some(mode.into()),
            Err(()) => {
                self.clear();
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procfs::Memory;
    use std::time::Duration;

    #[test]
    fn own_death_healing_gap_maximum_and_failure() {
        let mut h = History::default();
        let base = Instant::now();
        let t = |ms| base + Duration::from_millis(ms);
        assert_eq!(h.taken(Some((100.0, 100.0)), t(0)), Some(None));
        assert_eq!(h.taken(Some((80.0, 100.0)), t(250)), Some(Some(20.0)));
        assert_eq!(h.taken(Some((90.0, 100.0)), t(499)), Some(None));
        assert_eq!(h.taken(Some((80.0, 100.0)), t(998)), Some(Some(10.0)));
        assert_eq!(h.taken(Some((70.0, 100.0)), t(1498)), Some(None));
        assert_eq!(h.taken(Some((60.0, 120.0)), t(1750)), Some(None));
        assert_eq!(h.taken(Some((0.0, 120.0)), t(2000)), Some(Some(60.0)));
        assert_eq!(h.taken(Some((100.0, 120.0)), t(2250)), Some(None));
        assert_eq!(h.taken(None, t(2500)), None);
        assert_eq!(h.taken(Some((80.0, 120.0)), t(2750)), Some(None));
        for value in [f64::NAN, f64::INFINITY, -1.0, 121.0] {
            assert_eq!(h.taken(Some((value, 120.0)), t(3000)), None);
            assert_eq!(h.taken(Some((80.0, 120.0)), t(3250)), Some(None));
        }
    }

    #[test]
    fn target_channels_merge_serial_reuse_and_resets() {
        let mut h = History::default();
        let base = Instant::now();
        let t = |ms| base + Duration::from_millis(ms);
        let (a, b) = ((1, 7), (2, 9));
        assert_eq!(h.dealt(&[(a, Some((100.0, 100.0, 50.0))), (b, Some((100.0, 100.0, 50.0)))], t(0)), (None, None));
        assert_eq!(
            h.dealt(&[(a, Some((80.0, 100.0, 40.0))), (b, Some((90.0, 100.0, 30.0)))], t(250)),
            (Some(30.0), Some(30.0))
        );
        assert_eq!(h.dealt(&[(a, Some((0.0, 100.0, 0.0))), (b, None)], t(499)), (Some(80.0), Some(40.0)));
        let c = (1, 8);
        assert_eq!(h.dealt(&[(c, Some((50.0, 100.0, 20.0)))], t(750)), (None, None));
        assert_eq!(h.dealt(&[(c, Some((f64::NAN, 100.0, 10.0)))], t(1000)), (None, Some(10.0)));
        assert_eq!(h.dealt(&[(c, Some((40.0, 100.0, 0.0)))], t(1250)), (None, Some(10.0)));
        assert_eq!(h.dealt(&[(c, Some((30.0, 120.0, 20.0)))], t(1500)), (None, None));
        h.reset(Feature::Dealt);
        assert_eq!(h.dealt(&[(c, Some((20.0, 120.0, 10.0)))], t(1750)), (None, None));
    }

    #[derive(Default)]
    struct Bytes {
        bytes: HashMap<u64, u8>,
        log: Vec<(u64, usize)>,
        mutate: bool,
    }

    impl Bytes {
        fn put(&mut self, addr: u64, data: &[u8]) {
            for (i, byte) in data.iter().enumerate() {
                self.bytes.insert(addr + i as u64, *byte);
            }
        }
        fn q(&mut self, addr: u64, value: u64) {
            self.put(addr, &value.to_le_bytes());
        }
        fn i(&mut self, addr: u64, value: i32) {
            self.put(addr, &value.to_le_bytes());
        }
        fn f(&mut self, addr: u64, value: f32) {
            self.put(addr, &value.to_le_bytes());
        }
    }

    impl Memory for Bytes {
        fn read(&mut self, addr: u64, size: usize) -> Result<Vec<u8>, i32> {
            self.log.push((addr, size));
            if self.mutate && addr == 0x300000 && self.log.iter().filter(|e| **e == (addr, size)).count() == 2 {
                self.i(addr + 16, 8);
            }
            (0..size).map(|i| self.bytes.get(&(addr + i as u64)).copied().ok_or(libc::EFAULT)).collect()
        }
        fn count(&self) -> u64 {
            self.log.len() as u64
        }
    }

    fn fixture() -> Bytes {
        let mut m = Bytes::default();
        let base = 0x100000000;
        m.i(base + 0xC337DD4, 4);
        m.q(base + 0xC337DC0, 0x200000);
        m.q(0x200000, 0x300000 - 24);
        m.put(0x300000, &[0; 24]);
        m.q(0x300000, 0x400000);
        m.i(0x300010, 7);
        m.q(base + 0xC16A450, 0x600000);
        m.q(0x400010, 0x600000);
        m.q(0x401400, 0x500000);
        m.q(base + 0xC146A88, 0x700000);
        m.q(0x500010, 0x700000);
        m.q(0x5000B0, 0x800000);
        m.f(0x800000, 80.0);
        m.f(0x5000CC, 100.0);
        m.f(0x50032C, 40.0);
        m
    }

    fn sample(m: &mut Bytes, key: Key) -> Result<Sample, ()> {
        let build = crate::meter::find_build((1793384724, 220024832)).unwrap();
        target(&mut Attached { mem: m, base: 0x100000000, build }, key)
    }

    #[test]
    fn weak_serial_flags_and_post_read_recheck() {
        let mut m = fixture();
        assert_eq!(sample(&mut m, (1, 7)), Ok((80.0, 100.0, 40.0)));
        assert_eq!(sample(&mut m, (1, 8)), Err(()));
        for flag in [0x200000, 0x10000000, 0x20000000] {
            m.i(0x300008, flag);
            assert_eq!(sample(&mut m, (1, 7)), Err(()));
        }
        m.i(0x300008, 0);
        m.log.clear();
        m.mutate = true;
        assert_eq!(sample(&mut m, (1, 7)), Err(()));
    }

    #[test]
    fn weak_bound_and_class_depth() {
        let mut m = fixture();
        assert_eq!(sample(&mut m, (4, 7)), Err(()));
        assert_eq!(m.log, [(0x10C337DD4, 4)]);
        for depth in [4, 5] {
            let mut cls = 0x600000;
            for i in 0..depth {
                let child = 0x900000 + i * 0x1000;
                m.q(child + 0x40, cls);
                cls = child;
            }
            m.q(0x400010, cls);
            assert_eq!(sample(&mut m, (1, 7)).is_ok(), depth == 4);
        }
    }

    #[test]
    fn float_payload_needs_only_four_byte_alignment() {
        let mut m = fixture();
        m.q(0x5000B0, 0x800004);
        m.f(0x800004, 70.0);
        assert_eq!(sample(&mut m, (1, 7)), Ok((70.0, 100.0, 40.0)));
    }

    fn own_fixture() -> Bytes {
        let mut m = fixture();
        let base = 0x100000000;
        let build = crate::meter::find_build((1793384724, 220024832)).unwrap();
        for (slot, cls) in [
            (build.cls_world, 0x610000),
            (build.cls_game_instance, 0x620000),
            (build.cls_local_player, 0x630000),
            (build.cls_player_controller, 0x640000),
            (build.cls_dune_vehicle, 0x650000),
            (build.cls_dune_player_character, 0x660000),
        ] {
            m.q(base + slot, cls);
        }
        for (obj, cls) in [
            (0xA00000, 0x610000),
            (0xA10000, 0x620000),
            (0xA30000, 0x630000),
            (0xA40000, 0x670000),
            (0xA50000, 0x650000),
            (0xA60000, 0x680000),
            (0x400000, 0x660000),
        ] {
            m.q(obj + 0x10, cls);
        }
        m.q(0x660040, 0x600000);
        m.q(0x600040, 0);
        m.q(0x670040, 0x640000);
        m.q(base + build.gworld, 0xA00000);
        m.q(0xA003C8, 0xA10000);
        m.q(0xA10038, 0xA20000);
        m.i(0xA10040, 1);
        m.q(0xA20000, 0xA30000);
        m.q(0xA30030, 0xA40000);
        m.q(0xA40388, 0x400000);
        m.q(0xA403F0, 0x400000);
        m.q(base + 0xC175948, 0x670000);
        m.q(base + 0xC1CD1D8, 0x680000);
        m.q(0xA41048, 0xA60000);
        m.q(0xA60268, 0x400000);
        m
    }

    #[test]
    fn own_route_vehicle_world_image_and_old_build() {
        let mut m = own_fixture();
        let base = 0x100000000;
        let build = crate::meter::find_build((1793384724, 220024832)).unwrap();
        let start = Instant::now();
        let mut r = DamageReader::default();
        let poll = |r: &mut DamageReader, m: &mut Bytes| r.taken(&mut Attached { mem: m, base, build });
        r.prepare(start, Features { taken: true, ..Features::NONE }, 1);
        assert_eq!(r.mode(&mut Attached { mem: &mut m, base, build }), Some("on foot".into()));
        assert_eq!(poll(&mut r, &mut m), Some(None));
        m.f(0x800000, 60.0);
        m.q(0xA40388, 0xA50000);
        r.prepare(start + Duration::from_millis(250), Features { taken: true, ..Features::NONE }, 1);
        assert_eq!(r.mode(&mut Attached { mem: &mut m, base, build }), Some("vehicle".into()));
        assert_eq!(poll(&mut r, &mut m), Some(Some(20.0)));
        m.q(0xB00010, 0x610000);
        m.q(0xB003C8, 0xA10000);
        m.q(base + build.gworld, 0xB00000);
        m.f(0x800000, 40.0);
        r.prepare(start + Duration::from_millis(499), Features { taken: true, ..Features::NONE }, 1);
        assert_eq!(poll(&mut r, &mut m), Some(None));
        m.f(0x800000, 30.0);
        r.prepare(start + Duration::from_millis(750), Features { taken: true, ..Features::NONE }, 2);
        assert_eq!(poll(&mut r, &mut m), Some(None));
        m.log.clear();
        let old = crate::meter::find_build((1793030166, 219996160)).unwrap();
        assert_eq!(r.taken(&mut Attached { mem: &mut m, base, build: old }), None);
        assert!(m.log.is_empty());
    }

    #[test]
    fn meter_clock_gap_and_route_rewalk() {
        use crate::meter::{Meter, Poll};
        use crate::procfs::Source;
        use std::cell::{Cell, RefCell};
        use std::io;
        use std::rc::Rc;
        struct Shared(Rc<RefCell<Bytes>>);
        impl Memory for Shared {
            fn read(&mut self, addr: u64, size: usize) -> Result<Vec<u8>, i32> {
                self.0.borrow_mut().read(addr, size)
            }
            fn count(&self) -> u64 {
                self.0.borrow().count()
            }
        }
        struct Process;
        impl Source for Process {
            fn pids(&self) -> Vec<i32> {
                vec![123456]
            }
            fn uid(&self, _: i32) -> io::Result<u32> {
                Ok(crate::procfs::me().1)
            }
            fn cmdline(&self, _: i32) -> io::Result<Vec<u8>> {
                Ok(crate::meter::EXE.as_bytes().to_vec())
            }
            fn maps(&self, _: i32) -> io::Result<String> {
                Ok(format!("100000000-100001000 r--p 00000000 00:00 1 /game/{}", crate::meter::EXE))
            }
            fn ppid(&self, _: i32) -> io::Result<i32> {
                Ok(0)
            }
            fn ptrace_scope(&self) -> Option<i64> {
                Some(0)
            }
        }
        let mut bytes = own_fixture();
        let base = 0x100000000;
        bytes.put(base, &[0; 1024]);
        bytes.put(base, b"MZ");
        bytes.i(base + 0x3C, 0x80);
        bytes.put(base + 0x80, b"PE\0\0");
        bytes.put(base + 0x84, &0x8664u16.to_le_bytes());
        bytes.i(base + 0x88, 1793384724);
        bytes.put(base + 0x98, &0x20Bu16.to_le_bytes());
        bytes.i(base + 0xD0, 220024832);
        let memory = Rc::new(RefCell::new(bytes));
        let factory = memory.clone();
        let start = Instant::now();
        let clock = Rc::new(Cell::new(start));
        let ticks = clock.clone();
        let mut meter =
            Meter::new(Box::new(Process), Box::new(move |_| Box::new(Shared(factory.clone()))), false, Duration::ZERO)
                .with_damage(Box::new(DamageReader::default()))
                .with_clock(Box::new(move || ticks.get()));
        let on = Features { taken: true, ..Features::NONE };
        assert_eq!(meter.poll_enabled(on).taken, Some(None));
        clock.set(start + Duration::from_millis(600));
        memory.borrow_mut().f(0x800000, 60.0);
        assert_eq!(meter.poll_enabled(on).taken, Some(None));
        clock.set(start + Duration::from_millis(850));
        memory.borrow_mut().f(0x800000, 50.0);
        assert_eq!(meter.poll_enabled(on).taken, Some(Some(10.0)));
        let gworld = crate::meter::find_build((1793384724, 220024832)).unwrap().gworld;
        assert_eq!(memory.borrow().log.iter().filter(|e| **e == (base + gworld, 8)).count(), 3);
    }
}
