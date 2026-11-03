"""Net-drop histories and weak-object validation, without a game or display."""
import errno
import math
import os
import struct
import unittest

import support  # noqa: F401  (the repository on sys.path)
import dehulud_combat as dehulud


class History(unittest.TestCase):
    def test_own_death_heal_maximum_gap_and_failure(self):
        h = dehulud.DamageHistory()
        self.assertEqual(h.taken((100, 100), 0), ())
        self.assertEqual(h.taken((80, 100), .25), (20,))
        self.assertEqual(h.taken((90, 100), .499), ())
        self.assertEqual(h.taken((80, 100), .998), (10,))
        self.assertEqual(h.taken((70, 100), 1.498), ())
        self.assertEqual(h.taken((60, 120), 1.75), ())
        self.assertEqual(h.taken((0, 120), 2), (60,))
        self.assertEqual(h.taken((100, 120), 2.25), ())
        self.assertIsNone(h.taken(None, 2.5))
        self.assertEqual(h.taken((80, 120), 2.75), ())
        for value in (math.nan, math.inf, -1, 121):
            self.assertIsNone(h.taken((value, 120), 3))
            self.assertEqual(h.taken((80, 120), 3.25), ())

    def test_target_identity_channels_merge_and_reset(self):
        h = dehulud.DamageHistory()
        a, b = (1, 7), (2, 9)
        self.assertEqual(h.dealt({a: (100, 100, 50), b: (100, 100, 50)}, 0), ((), ()))
        self.assertEqual(h.dealt({a: (80, 100, 40), b: (90, 100, 30)}, .25), ((30,), (30,)))
        self.assertEqual(h.dealt({a: (0, 100, 0), b: None}, .499), ((80,), (40,)))
        self.assertEqual(h.dealt({(1, 8): (50, 100, 20)}, .75), ((), ()))
        self.assertEqual(h.dealt({(1, 8): (math.nan, 100, 10)}, 1), ((), (10,)))
        self.assertEqual(h.dealt({(1, 8): (40, 100, 0)}, 1.25), ((), (10,)))
        self.assertEqual(h.dealt({(1, 8): (30, 120, 20)}, 1.5), ((), ()))
        h.reset("damage_dealt")
        self.assertEqual(h.dealt({(1, 8): (20, 120, 10)}, 1.75), ((), ()))
        h.begin((1, 2, 3))
        self.assertEqual(h.dealt({(1, 8): (10, 120, 0)}, 2), ((), ()))


class Memory:
    def __init__(self):
        self.bytes = {}
        self.log = []
        self.change_item = None

    def put(self, address, fmt, *values):
        for i, value in enumerate(struct.pack("<" + fmt, *values)):
            self.bytes[address + i] = value

    def read(self, address, size):
        self.log.append((address, size))
        if self.change_item == address and self.log.count((address, size)) == 2:
            self.put(address + 16, "i", 8)
        try:
            return bytes(self.bytes[address + i] for i in range(size))
        except KeyError:
            raise OSError(errno.EFAULT, "unmapped test address")

    @property
    def count(self):
        return len(self.log)


class WeakTargets(unittest.TestCase):
    def setUp(self):
        self.reader = dehulud.DamageReader()
        self.mem = m = Memory()
        self.game = dehulud.Attached(0x100000000, dehulud.BUILDS[(1793384724, 220024832)], m)
        self.item, self.obj, self.attrs = 0x300000, 0x400000, 0x500000
        m.put(self.game.base + 0xC337DD4, "i", 4)
        m.put(self.game.base + 0xC337DC0, "Q", 0x200000)
        m.put(0x200000, "Q", self.item - 24)
        m.put(self.item, "Q I 4x i 4x", self.obj, 0, 7)
        m.put(self.game.base + 0xC16A450, "Q", 0x600000)
        m.put(self.obj + 0x10, "Q", 0x600000)
        m.put(self.obj + 0x1400, "Q", self.attrs)
        m.put(self.game.base + 0xC146A88, "Q", 0x700000)
        m.put(self.attrs + 0x10, "Q", 0x700000)
        m.put(self.attrs + 0x98 + 0x18, "Q", 0x800000)
        m.put(0x800000, "f", 80)
        m.put(self.attrs + 0xC0 + 12, "f", 100)
        m.put(self.attrs + 0x320 + 12, "f", 40)

    def test_serial_flags_and_post_read_recheck(self):
        self.assertEqual(self.reader.target(self.game, (1, 7)), (80, 100, 40))
        self.assertIsNone(self.reader.target(self.game, (1, 8)))
        for flag in (0x200000, 0x10000000, 0x20000000):
            self.mem.put(self.item + 8, "I", flag)
            self.assertIsNone(self.reader.target(self.game, (1, 7)))
        self.mem.put(self.item + 8, "I", 0)
        self.mem.log.clear()
        self.mem.change_item = self.item
        self.assertIsNone(self.reader.target(self.game, (1, 7)))

    def test_count_bound_stops_before_chunk_read(self):
        self.assertIsNone(self.reader.target(self.game, (4, 7)))
        self.assertEqual(self.mem.log, [(self.game.base + 0xC337DD4, 4)])

    def test_float_payload_needs_only_four_byte_alignment(self):
        self.mem.put(self.attrs + 0x98 + 0x18, "Q", 0x800004)
        self.mem.put(0x800004, "f", 70)
        self.assertEqual(self.reader.target(self.game, (1, 7)), (70, 100, 40))

    def test_class_depth_four_but_not_five(self):
        for depth in (4, 5):
            cls = 0x600000
            for i in range(depth):
                child = 0x900000 + i * 0x1000
                self.mem.put(child + 0x40, "Q", cls)
                cls = child
            self.mem.put(self.obj + 0x10, "Q", cls)
            if depth == 4:
                self.assertEqual(self.reader.target(self.game, (1, 7)), (80, 100, 40))
            else:
                with self.assertRaises(dehulud._Soft):
                    self.reader.target(self.game, (1, 7))


class TargetRoles(unittest.TestCase):
    def test_role_change_killing_blow_dedup_expiry_and_switch_reset(self):
        r = dehulud.DamageReader()
        roles = [None, None]
        values = {}
        observed = []
        r.character = lambda game: (1, 0x100000, "on foot")
        r.handle = lambda game, address: roles[0 if address == 0x101E58 else 1]
        def target(game, key):
            observed.append(key)
            return values[key]
        r.target = target
        def poll(at, aim, soft):
            r.begin(at, {"damage_dealt"}, 1)
            roles[:] = aim, soft
            observed.clear()
            return r.dealt(dehulud.Attached(0, dehulud.BUILDS[(1793384724, 220024832)], None))
        a, b = (1, 1), (2, 1)
        values[a] = (100, 100, 50)
        self.assertEqual(poll(0, a, a), ((), ()))
        self.assertEqual(observed, [a])
        values[a] = (0, 100, 0)
        self.assertEqual(poll(.25, None, a), ((100,), (50,)))
        values[b] = (100, 100, 50)
        poll(.5, b, None)
        values[b] = (80, 100, 40)
        self.assertEqual(poll(.75, None, None), ((20,), (10,)))
        r.reset("damage_dealt")
        values[b] = (60, 100, 30)
        self.assertEqual(poll(1, b, None), ((), ()))
        self.assertEqual(poll(19, None, None), ((), ()))
        self.assertEqual(observed, [])

    def test_four_prior_targets_and_six_target_ceiling(self):
        r = dehulud.DamageReader()
        roles, seen = [None, None], []
        r.character = lambda game: (1, 0x100000, "on foot")
        r.handle = lambda game, address: roles[0 if address == 0x101E58 else 1]
        def target(game, key):
            seen.append(key)
            return (100, 100, 50)
        r.target = target
        for index in range(7):
            r.begin(index * .25, {"damage_dealt"}, 1)
            roles[:] = (index, 1), (99, 1)
            seen.clear()
            r.dealt(dehulud.Attached(0, dehulud.BUILDS[(1793384724, 220024832)], None))
        self.assertEqual(seen, [(6, 1), (99, 1), (5, 1), (4, 1), (3, 1), (2, 1)])


class ReaderRoutes(unittest.TestCase):
    def setUp(self):
        WeakTargets.setUp(self)
        m, b = self.mem, self.game.build
        classes = {"cls_world": 0x610000, "cls_game_instance": 0x620000,
                   "cls_local_player": 0x630000, "cls_player_controller": 0x640000,
                   "cls_dune_vehicle": 0x650000, "cls_dune_player_character": 0x660000}
        for key, cls in classes.items():
            m.put(self.game.base + b[key], "Q", cls)
        for obj, cls in ((0xA00000, 0x610000), (0xA10000, 0x620000),
                         (0xA30000, 0x630000), (0xA40000, 0x670000),
                         (0xA50000, 0x650000), (0xA60000, 0x680000),
                         (self.obj, 0x660000)):
            m.put(obj + 0x10, "Q", cls)
        m.put(0x660040, "Q", 0x600000)
        m.put(0x600040, "Q", 0)
        m.put(0x670040, "Q", 0x640000)
        m.put(self.game.base + b["gworld"], "Q", 0xA00000)
        m.put(0xA003C8, "Q", 0xA10000)
        m.put(0xA10038, "Qi", 0xA20000, 1)
        m.put(0xA20000, "Q", 0xA30000)
        m.put(0xA30030, "Q", 0xA40000)
        m.put(0xA40388, "Q", self.obj)
        m.put(0xA403F0, "Q", self.obj)
        m.put(self.game.base + 0xC175948, "Q", 0x670000)
        m.put(self.game.base + 0xC1CD1D8, "Q", 0x680000)
        m.put(0xA41048, "Q", 0xA60000)
        m.put(0xA60268, "Q", self.obj)

    def test_own_route_vehicle_world_image_and_old_build(self):
        r, m = self.reader, self.mem
        r.begin(0, {"damage_taken"}, 1)
        self.assertEqual(r.mode(self.game), "on foot")
        self.assertEqual(r.taken(self.game), ())
        m.put(0x800000, "f", 60)
        m.put(0xA40388, "Q", 0xA50000)
        r.begin(.25, {"damage_taken"}, 1)
        self.assertEqual(r.mode(self.game), "vehicle")
        self.assertEqual(r.taken(self.game), (20,))
        # A new World with the same GameInstance is still a fresh baseline.
        m.put(0xB00010, "Q", 0x610000)
        m.put(0xB003C8, "Q", 0xA10000)
        m.put(self.game.base + self.game.build["gworld"], "Q", 0xB00000)
        m.put(0x800000, "f", 40)
        r.begin(.499, {"damage_taken"}, 1)
        self.assertEqual(r.taken(self.game), ())
        m.put(0x800000, "f", 30)
        r.begin(.75, {"damage_taken"}, 2)
        self.assertEqual(r.taken(self.game), ())
        old = dehulud.Attached(self.game.base, dehulud.BUILDS[(1793030166, 219996160)], m)
        m.log.clear()
        self.assertIsNone(r.taken(old))
        self.assertEqual(m.log, [])

    def test_meter_clock_gap_and_route_rewalk(self):
        m, g = self.mem, self.game
        m.put(g.base, "1024s", b"")
        m.put(g.base, "2s", b"MZ")
        m.put(g.base + 0x3C, "I", 0x80)
        m.put(g.base + 0x80, "4sH2xI", b"PE\0\0", 0x8664, 1793384724)
        m.put(g.base + 0x98, "H", 0x20B)
        m.put(g.base + 0xD0, "I", 220024832)
        class Source:
            def pids(self): return [123456]
            def uid(self, pid): return os.getuid()
            def cmdline(self, pid): return dehulud.EXE.encode()
            def maps(self, pid): return "%x-%x r--p 00000000 00:00 1 /game/%s" % (g.base, g.base + 4096, dehulud.EXE)
        now = [0]
        meter = dehulud.Meter(source=Source(), reader_factory=lambda pid: m,
                              damage=self.reader, clock=lambda: now[0])
        self.assertEqual(meter.poll({"damage_taken"}).taken, ())
        now[0] = .6
        m.put(0x800000, "f", 60)
        self.assertEqual(meter.poll({"damage_taken"}).taken, ())
        now[0] = .85
        m.put(0x800000, "f", 50)
        self.assertEqual(meter.poll({"damage_taken"}).taken, (10,))
        self.assertEqual(m.log.count((g.base + g.build["gworld"], 8)), 3)
