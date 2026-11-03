"""Independent byte walks of the bound fake game's real route shape."""
import struct
import unittest

from damage_model import LoggedReader
from support import FakeGame, FakeSource
import dehulud_combat as dehulud


class BoundRouteTests(unittest.TestCase):
    def setUp(self):
        self.game = FakeGame("--build", "26333948", "--damage")
        self.addCleanup(self.game.close)
        self.reader = LoggedReader(dehulud.Reader(self.game.pid))

    def value(self, address, fmt):
        return struct.unpack("<" + fmt, self.reader.read(address, struct.calcsize("<" + fmt)))[0]

    def pointer(self, address):
        return self.value(address,"Q")

    def class_is(self, address, expected):
        cls = self.pointer(address + 0x10)
        for _ in range(5):
            if cls == expected:
                return True
            if not cls:
                break
            cls = self.pointer(cls + 0x40)
        return False

    def controller(self):
        world = self.pointer(self.game.base + 0xC4D3528)
        self.assertEqual(self.pointer(world + 0x10),self.pointer(self.game.base + 0xC4D2F00))
        gi = self.pointer(world + 0x3C8)
        self.assertTrue(self.class_is(gi,self.pointer(self.game.base + 0xC4A1BC0)))
        self.assertGreater(self.value(gi + 0x38 + 8,"i"),0)
        lp = self.pointer(self.pointer(gi + 0x38))
        self.assertTrue(self.class_is(lp,self.pointer(self.game.base + 0xC4B1730)))
        pc = self.pointer(lp + 0x30)
        self.assertTrue(self.class_is(pc,self.pointer(self.game.base + 0xC4BF0E8)))
        return pc

    def own(self):
        pc = self.controller()
        pawn = self.pointer(pc + 0x388)
        if self.class_is(pawn,self.pointer(self.game.base + 0xC1E7D88)):
            self.assertTrue(self.class_is(pc,self.pointer(self.game.base + 0xC175948)))
            inventory = self.pointer(pc + 0x1048)
            self.assertTrue(self.class_is(inventory,self.pointer(self.game.base + 0xC1CD1D8)))
            pawn = self.pointer(inventory + 0x268)
        else:
            self.assertEqual(pawn,self.pointer(pc + 0x3F0))
        self.assertTrue(self.class_is(pawn,self.pointer(self.game.base + 0xC16A450)))
        self.assertTrue(self.class_is(pawn,self.pointer(self.game.base + 0xC1732B0)))
        return pawn

    def vitals(self, character):
        attrs = self.pointer(character + 0x1400)
        self.assertEqual(self.pointer(attrs + 0x10),self.pointer(self.game.base + 0xC146A88))
        return (self.value(self.pointer(attrs + 0x98 + 0x18),"f"),
                self.value(attrs + 0xC0 + 0xC,"f"),self.value(attrs + 0x320 + 0xC,"f"))

    def target(self):
        own = self.own()
        index,serial = struct.unpack("<ii",self.reader.read(own + 0x1E58,8))
        count = self.value(self.game.base + 0xC337DD4,"i")
        if not 0 <= index < count or not serial:
            return None
        chunks = self.pointer(self.game.base + 0xC337DC0)
        chunk = self.pointer(chunks + (index >> 16) * 8)
        if not chunk:
            return None
        item = chunk + (index & 0xFFFF) * 24
        before = self.reader.read(item,24)
        address,flags,actual_serial = struct.unpack_from("<QI4xi",before)
        if not address or flags & 0x30200000 or actual_serial != serial:
            return None
        if not self.class_is(address,self.pointer(self.game.base + 0xC16A450)):
            return None
        result = self.vitals(address)
        return result if self.reader.read(item,24) == before else None

    def frame(self, **frame):
        return self.game.damage_frame(frame,self.reader)

    def test_routes_mutation_and_real_chunk_boundary(self):
        self.assertEqual(self.game.damage["bound_build"],"26333948")
        self.assertEqual(self.vitals(self.own()),(100,100,50))
        self.assertEqual(self.target(),(100,100,50))
        self.frame(vitals={"own":{"health":80,"maximum":120},"enemy2":{"health":35,"shield":9}},
                   aim="enemy2",soft_lock="enemy")
        self.assertEqual(self.vitals(self.own()),(80,120,50))
        self.assertEqual(self.target(),(35,100,9))
        self.assertEqual(self.game.damage["handles"]["enemy2"],[65537,1])
        self.frame(pawn="vehicle")
        own = self.own()
        self.reader.assert_no_reads(["damage_taken","damage_dealt","worm"],self.game.damage["feature_ranges"])
        self.assertEqual(self.vitals(own),(80,120,50))
        meter = dehulud.Meter(source=FakeSource(self.game),scan_interval=0)
        self.assertEqual(meter.poll(frozenset(["worm"])).percent,80)

    def test_all_weak_guards_and_class_depth(self):
        cases = [({"aim":[1,999]},None),({"aim":[65560,1]},None),({"aim":[-1,1]},None),
                 ({"aim":"vehicle"},None),({"aim":"other"},None),
                 ({"aim":"enemy","items":[{"index":1,"object":None,"serial":1}]},None)]
        for frame,expected in cases:
            self.frame(**frame)
            self.assertEqual(self.target(),expected,frame)
        for flag in (0x00200000,0x10000000,0x20000000):
            self.frame(aim="enemy",spawn=[{"name":"enemy","index":1,"serial":1,"flags":flag}])
            self.assertIsNone(self.target())
        for depth in (4,5):
            self.frame(aim="enemy",spawn=[{"name":"enemy","index":1,"serial":1,"depth":depth}])
            self.assertEqual(self.target(),(100,100,50) if depth == 4 else None)

    def test_real_identity_changes_failure_ranges_and_feature_reads(self):
        old_world = self.pointer(self.game.base + 0xC4D3528)
        self.frame(world=2,vitals={"own":{"health":80}})
        self.assertNotEqual(self.pointer(self.game.base + 0xC4D3528),old_world)
        self.assertEqual(self.vitals(self.own()),(80,100,50))
        old_base = self.game.base
        old_ident = self.reader.read(old_base,0x400)
        self.frame(image=2)
        self.assertNotEqual(self.game.base,old_base)
        self.assertNotEqual(self.reader.read(old_base,0x400),old_ident)
        self.assertEqual(dehulud.pe_identity(self.reader.read(self.game.base,0x400)),(1793384724,220024832))
        self.assertEqual(self.vitals(self.own()),(80,100,50))
        self.frame(fail_reads=[["own","health"]])
        with self.assertRaises(OSError):
            self.vitals(self.own())
        self.frame()
        self.controller()
        self.reader.assert_no_reads(["damage_taken","damage_dealt","worm"],self.game.damage["feature_ranges"])
        self.vitals(self.own())
        with self.assertRaises(AssertionError):
            self.reader.assert_no_reads(["damage_taken"],self.game.damage["feature_ranges"])

    def test_unpossessed_vehicle_class_is_dealt_only(self):
        self.frame(aim="vehicle")
        self.reader.read(self.game.damage["objects"]["vehicle"] + 0x10,8)
        with self.assertRaises(AssertionError):
            self.reader.assert_no_reads(["damage_dealt"],self.game.damage["feature_ranges"])
        self.frame(pawn="vehicle")
        self.own()
        self.reader.assert_no_reads(["damage_dealt"],self.game.damage["feature_ranges"])
