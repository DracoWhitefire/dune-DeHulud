"""Shared damage frames through the public meter and real child-process memory."""
import json
import os
import unittest

from damage_model import LoggedReader
from support import FakeGame, FakeSource, HERE
import dehulud_combat as dehulud


with open(os.path.join(HERE, "damage_scenarios.json")) as fixture:
    SCENARIOS = json.load(fixture)["scenarios"]


class DamageScenarios(unittest.TestCase):
    def run_scenario(self, scenario):
        game = FakeGame("--build", "26333948", "--damage")
        self.addCleanup(game.close)
        reader = LoggedReader(dehulud.Reader(game.pid))
        class AttachmentReader:
            def __init__(self, pid):
                self.count = 0

            def read(self, address, size):
                self.count += 1
                return reader.read(address, size)

        now = [0.0]
        meter = dehulud.Meter(source=FakeSource(game), reader_factory=AttachmentReader,
                              scan_interval=0, clock=lambda: now[0], damage=dehulud.DamageReader())
        enabled = dehulud.ALL
        for frame in scenario["frames"]:
            with self.subTest(at_ms=frame["at_ms"]):
                metadata = game.damage_frame(frame, reader)
                updated = set(enabled)
                for key, on in frame.get("switches", {}).items():
                    if on:
                        updated.add(key)
                    else:
                        updated.discard(key)
                updated = frozenset(updated)
                for key in enabled ^ updated:
                    meter.reset(key)
                enabled = updated
                now[0] = frame["at_ms"] / 1000.0
                reading = meter.poll(enabled)
                health, shield = reading.dealt if reading.dealt is not None else ((), ())
                observed = {"taken": list(reading.taken or ()), "health": list(health), "shield": list(shield)}
                self.assertEqual(observed, frame["expected"])
                disabled = dehulud.ALL - enabled
                self.assertTrue(set(frame.get("no_reads", [])) <= disabled)
                reader.assert_no_reads(disabled, metadata["feature_ranges"])
                if not enabled:
                    self.assertEqual(reader.log, [])
                if "image" in frame and meter.game is not None:
                    self.assertEqual(meter.game[1], game.base)


def scenario_test(scenario):
    def test(self):
        self.run_scenario(scenario)
    return test


for scenario in SCENARIOS:
    setattr(DamageScenarios, "test_" + scenario["name"], scenario_test(scenario))
