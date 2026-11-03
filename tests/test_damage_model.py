"""Verify the test arena independently of either application's damage detector."""
import json
import os
import struct
import unittest

from damage_model import DamageModel, LoggedReader, DEFAULT_LAYOUT
from support import FakeGame
import dehulud_combat as dehulud


def unpack(reader, address, fmt):
    return struct.unpack("<" + fmt, reader.read(address, struct.calcsize("<" + fmt)))[0]


def resolve(model, index, serial):
    """Test-side traversal of bytes, deliberately not DamageModel.item()."""
    l, read = model.layout, model.reader
    table = model.objects["table"]
    count = unpack(read, table + l["count"], "i")
    if not 0 <= index < count:
        return None
    chunks = unpack(read, table + l["chunks"], "Q")
    chunk = unpack(read, chunks + (index >> l["shift"]) * 8, "Q")
    if not chunk:
        return None
    item = chunk + (index & ((1 << l["shift"]) - 1)) * l["item_stride"]
    if unpack(read, item + l["item_serial"], "i") != serial:
        return None
    return unpack(read, item + l["item_object"], "Q") or None


class ModelTests(unittest.TestCase):
    def setUp(self):
        self.model = DamageModel()
        self.addCleanup(self.model.close)

    def test_chunk_boundaries_null_serial_and_range(self):
        m = self.model
        for index in (7, 8, 23):
            m.character("edge%d" % index, index, 40 + index)
            self.assertEqual(resolve(m, index, 40 + index), m.objects["edge%d" % index])
        for index, serial in ((-1, 1), (24, 1), (1, 2), (6, 0)):
            self.assertIsNone(resolve(m, index, serial))
        m.table_entry(1, 0, 1)
        self.assertIsNone(resolve(m, 1, 1))
        m.put(m.objects["chunk_table"] + 8, "Q", 0)
        self.assertIsNone(resolve(m, 8, 48))

    def test_custom_layout_is_used(self):
        layout = dict(DEFAULT_LAYOUT, shift=2, item_stride=32, health=0x200, maximum=0x204, shield=0x208)
        m = DamageModel(layout)
        self.addCleanup(m.close)
        self.assertEqual(resolve(m, 9, 1), m.objects["enemy2"])
        self.assertEqual(unpack(m.reader, m.objects["enemy"] + 0x200, "f"), 100)

    def test_class_chains_zero_four_and_five(self):
        m = self.model
        for depth in (0, 4, 5):
            address = m.character("depth%d" % depth, 4, 1, depth=depth)
            cls = unpack(m.reader, address + m.layout["class"], "Q")
            steps = 0
            while cls != m.objects["character_class"]:
                cls = unpack(m.reader, cls + m.layout["super"], "Q")
                steps += 1
                self.assertLessEqual(steps, 5)
            self.assertEqual(steps, depth)
        self.assertNotEqual(unpack(m.reader, m.objects["vehicle"] + m.layout["class"], "Q"),
                            m.objects["character_class"])

    def test_frame_mutation_handles_identity_and_failures(self):
        m = self.model
        m.apply({"vitals":{"enemy":{"health":42,"maximum":120,"shield":9}},
                 "pawn":"vehicle", "aim":[1,99], "soft_lock":"enemy2", "world":2,"image":3,
                 "fail_reads":[["enemy","shield"]]})
        self.assertEqual((m.world,m.image),(2,3))
        r = m.reader
        r.route = "damage_dealt"
        self.assertEqual(unpack(r,m.objects["enemy"] + m.layout["health"],"f"),42)
        self.assertEqual(unpack(r,m.objects["enemy"] + m.layout["maximum"],"f"),120)
        with self.assertRaises(OSError):
            r.read(m.objects["enemy"] + m.layout["shield"] - 1, 2)
        self.assertEqual(r.log[-1]["route"],"damage_dealt")
        self.assertEqual(r.log[-1]["size"],2)
        self.assertEqual(struct.unpack("<ii",m.read(m.objects["controller"] + m.layout["aim"],8)),(1,99))
        self.assertEqual(unpack(r,m.objects["controller"] + m.layout["pawn"],"Q"),m.objects["vehicle"])
        m.apply({})
        self.assertEqual(r.log,[])
        self.assertEqual(unpack(r,m.objects["enemy"] + m.layout["shield"],"f"),9)

    def test_serial_reuse_does_not_alias_old_handle(self):
        m = self.model
        m.character("replacement",1,2,health=12)
        self.assertIsNone(resolve(m,1,1))
        self.assertEqual(resolve(m,1,2),m.objects["replacement"])

    def test_child_process_layout_is_readable_and_mutable(self):
        game = FakeGame("--damage")
        self.addCleanup(game.close)
        info = game.damage
        reader = LoggedReader(dehulud.Reader(game.pid))
        address = info["objects"]["enemy"] + info["layout"]["health"]
        self.assertEqual(unpack(reader,address,"f"),100)
        game.cmd('damage {"vitals":{"enemy":{"health":37}}}')
        self.assertEqual(unpack(reader,address,"f"),37)
        self.assertEqual([entry["address"] for entry in reader.log],[address,address])

    def test_disabled_range_catches_mislabelled_and_failed_reads(self):
        m = self.model
        address = m.objects["enemy"] + m.layout["health"]
        m.reader.route = "shared"
        m.reader.read(address, 4)
        with self.assertRaises(AssertionError):
            m.reader.assert_no_reads(["damage_dealt"], m.feature_ranges())
        m.reader.log.clear()
        m.reader.failures = [(address, address + 4)]
        with self.assertRaises(OSError):
            m.reader.read(address - 1, 2)
        with self.assertRaises(AssertionError):
            m.reader.assert_no_reads(["damage_dealt"], m.feature_ranges())
        m.reader.log.clear()
        m.reader.read(m.objects["controller"] + m.layout["pawn"], 8)
        m.reader.assert_no_reads(["damage_taken", "damage_dealt", "worm"], m.feature_ranges())

    def test_child_failure_metadata_applies_to_parent_reader(self):
        game = FakeGame("--damage")
        self.addCleanup(game.close)
        reader = LoggedReader(dehulud.Reader(game.pid))
        frame = {"fail_reads":[["enemy","health"]],"vitals":{"enemy":{"health":80}}}
        metadata = game.damage_frame(frame, reader)
        address = metadata["objects"]["enemy"] + metadata["layout"]["health"]
        self.assertEqual(metadata["failure_ranges"], [[address,address+4]])
        with self.assertRaises(OSError):
            reader.read(address,4)
        with self.assertRaises(AssertionError):
            reader.assert_no_reads(["damage_dealt"], metadata["feature_ranges"])
        game.damage_frame({}, reader)
        self.assertEqual(unpack(reader,address,"f"),80)

    def test_child_rejects_unbound_identity_keys_before_mutation(self):
        game = FakeGame("--damage")
        self.addCleanup(game.close)
        for key in ("world", "image"):
            with self.subTest(key=key):
                with self.assertRaisesRegex(ValueError, "unbound identity"):
                    game.damage_frame({key:2,"vitals":{"enemy":{"health":80}}})
                address = game.damage["objects"]["enemy"] + game.damage["layout"]["health"]
                self.assertEqual(unpack(dehulud.Reader(game.pid),address,"f"),100)
                self.assertEqual(game.damage[key],1)

    def test_child_worm_ranges_catch_reads_despite_shared_label(self):
        game = FakeGame("--damage")
        self.addCleanup(game.close)
        reader = LoggedReader(dehulud.Reader(game.pid))
        ranges = game.damage["feature_ranges"]
        self.assertEqual(len(ranges["worm"]),6)
        address = game.obj["char_comp"] + 0x334
        self.assertAlmostEqual(unpack(reader,address,"f"),42.5)
        with self.assertRaises(AssertionError):
            reader.assert_no_reads(["worm"],ranges)
        reader.log.clear()
        unpack(reader,game.obj["pc"] + 0x388,"Q")
        reader.assert_no_reads(["worm","damage_taken","damage_dealt"],ranges)


class ScenarioTests(unittest.TestCase):
    def test_all_scripts_apply_and_have_explicit_expectations(self):
        with open(os.path.join(os.path.dirname(__file__),"damage_scenarios.json")) as f:
            data = json.load(f)
        self.assertEqual(data["version"],1)
        names = [s["name"] for s in data["scenarios"]]
        self.assertEqual(len(names),len(set(names)))
        for scenario in data["scenarios"]:
            with self.subTest(scenario=scenario["name"]):
                model = DamageModel()
                try:
                    previous = -1
                    for frame in scenario["frames"]:
                        self.assertGreater(frame["at_ms"],previous)
                        previous = frame["at_ms"]
                        self.assertEqual(set(frame["expected"]),{"taken","health","shield"})
                        model.apply(frame)
                finally:
                    model.close()
