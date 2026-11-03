"""Test-only damage memory layout and reader instrumentation.

DEFAULT_LAYOUT is synthetic, not a supported game's offsets. Supply a complete
layout mapping when exercising a real build's reader. All memory belongs to the
test process; this module never discovers or modifies another process.
"""
import ctypes
import errno
import mmap
import struct


DEFAULT_LAYOUT = {
    "arena_size": 0x40000, "allocation_stride": 0x1000,
    "class": 0x10, "super": 0x40,
    "health": 0x100, "maximum": 0x104, "shield": 0x108,
    "pawn": 0x100, "own_character": 0x108, "aim": 0x110, "soft_lock": 0x118,
    "chunks": 0x100, "count": 0x108,
    "shift": 3, "item_stride": 24, "item_object": 0, "item_serial": 16,
    "capacity": 24,
}


class LoggedReader:
    """Wrap the actual reader, logging attempted reads including injected failures.

    Set route before a poll/route call. Native readers need equivalent logging at
    their read boundary: process_vm_readv does not notify the target process.
    """
    def __init__(self, reader):
        self.reader = reader
        self.log = []
        self.failures = []
        self.route = "shared"

    @property
    def count(self):
        return len(self.log)

    def read(self, address, size):
        self.log.append({"address": address, "size": size, "route": self.route})
        if any(address < end and start < address + size for start, end in self.failures):
            raise OSError(errno.EFAULT, "injected read failure")
        return self.reader.read(address, size)

    def assert_no_reads(self, disabled, feature_ranges):
        """Check exclusive address ranges, independent of consumer-supplied labels."""
        for feature in disabled:
            for start, end in feature_ranges[feature]:
                for entry in self.log:
                    if entry["address"] < end and start < entry["address"] + entry["size"]:
                        raise AssertionError("disabled %s read: %r" % (feature, entry))


class DamageModel:
    """Real mapped bytes for chunked weak handles, class chains and mutable vitals."""
    def __init__(self, layout=None):
        self.layout = dict(DEFAULT_LAYOUT if layout is None else layout)
        l = self.layout
        if l["shift"] < 0 or l["capacity"] <= 0:
            raise ValueError("invalid object table dimensions")
        self.memory = mmap.mmap(-1, l["arena_size"])
        self.base = ctypes.addressof(ctypes.c_char.from_buffer(self.memory))
        self.next_offset = 0
        self.objects = {}
        self.handles = {}
        self.worm_ranges = []
        self.own_name = "own"
        self.image = 1
        self.world = 1
        self.reader = LoggedReader(self)
        for name in ("character_class", "vehicle_class", "other_class", "controller", "table", "chunk_table"):
            self.allocate(name)
        n = (l["capacity"] + (1 << l["shift"]) - 1) >> l["shift"]
        self.chunks = [self.allocate("chunk%d" % i) for i in range(n)]
        for i, chunk in enumerate(self.chunks):
            self.put(self.objects["chunk_table"] + i * 8, "Q", chunk)
        self.put(self.objects["table"] + l["chunks"], "Q", self.objects["chunk_table"])
        self.put(self.objects["table"] + l["count"], "i", l["capacity"])
        self.character("own", 0, 1)
        self.character("enemy", 1, 1)
        self.character("enemy2", 9, 1)
        self.character("vehicle", 2, 1, kind="vehicle")
        self.character("other", 3, 1, kind="other")
        self.set_pawn("own")
        self.set_handle("aim", "enemy")
        self.set_handle("soft_lock", None)

    def close(self):
        self.memory.close()

    def allocate(self, name):
        off = self.next_offset
        self.next_offset += self.layout["allocation_stride"]
        if self.next_offset > len(self.memory):
            raise ValueError("test arena exhausted")
        self.objects[name] = self.base + off
        return self.objects[name]

    def put(self, address, fmt, value):
        struct.pack_into("<" + fmt, self.memory, address - self.base, value)

    def read(self, address, size):
        off = address - self.base
        if off < 0 or size < 0 or off + size > len(self.memory):
            raise OSError(errno.EFAULT, "outside test arena")
        return self.memory[off:off + size]

    def item(self, index):
        l = self.layout
        if not 0 <= index < l["capacity"]:
            raise ValueError("object index out of range")
        return self.chunks[index >> l["shift"]] + (index & ((1 << l["shift"]) - 1)) * l["item_stride"]

    def character(self, name, index, serial, health=100, maximum=100, shield=50, kind="character", depth=0):
        address = self.objects.get(name) or self.allocate(name)
        cls = self.objects[kind + "_class"]
        for level in range(depth):
            child = self.allocate(name + "_class_%d" % level)
            self.put(child + self.layout["super"], "Q", cls)
            cls = child
        self.put(address + self.layout["class"], "Q", cls)
        self.vitals(name, health=health, maximum=maximum, shield=shield)
        self.handles[name] = (index, serial)
        self.table_entry(index, address, serial)
        return address

    def table_entry(self, index, address, serial):
        item = self.item(index)
        self.put(item + self.layout["item_object"], "Q", address)
        self.put(item + self.layout["item_serial"], "i", serial)

    def vitals(self, name, **values):
        for key, value in values.items():
            self.put(self.objects[name] + self.layout[key], "f", float(value))

    def set_pawn(self, name, own="own"):
        self.own_name = own
        for field, target in (("pawn", name), ("own_character", own)):
            self.put(self.objects["controller"] + self.layout[field], "Q", self.objects[target] if target else 0)

    def set_handle(self, role, target):
        pair = self.handles[target] if isinstance(target, str) else target
        index, serial = pair if pair is not None else (-1, 0)
        address = self.objects["controller"] + self.layout[role]
        self.put(address, "i", index)
        self.put(address + 4, "i", serial)

    def apply(self, frame):
        """Apply one JSON-compatible frame; time and switches belong to the consumer."""
        for name, values in frame.get("vitals", {}).items():
            self.vitals(name, **values)
        for entry in frame.get("spawn", []):
            self.character(**entry)
        for role in ("aim", "soft_lock"):
            if role in frame:
                self.set_handle(role, frame[role])
        if "pawn" in frame:
            self.set_pawn(frame["pawn"], frame.get("own", "own"))
        for key in ("image", "world"):
            if key in frame:
                setattr(self, key, frame[key])
        self.reader.failures = [(self.objects[name] + self.layout[field],
                                 self.objects[name] + self.layout[field] + 4)
                                for name, field in frame.get("fail_reads", [])]
        self.reader.log.clear()

    def info(self):
        return {"base": self.base, "objects": self.objects, "layout": self.layout,
                "handles": self.handles, "image": self.image, "world": self.world,
                "failure_ranges": self.reader.failures, "feature_ranges": self.feature_ranges()}

    def feature_ranges(self):
        """Exclusive feature ranges; shared identity and pawn pointers are omitted."""
        l = self.layout
        stride = l["allocation_stride"]
        dealt = []
        for name in ("table", "chunk_table"):
            dealt.append((self.objects[name], self.objects[name] + stride))
        dealt.extend((chunk, chunk + stride) for chunk in self.chunks)
        for name in self.handles:
            if name != self.own_name:
                dealt.append((self.objects[name], self.objects[name] + stride))
        for field in ("aim", "soft_lock"):
            start = self.objects["controller"] + l[field]
            dealt.append((start, start + 8))
        taken = []
        if self.own_name is not None:
            for field in ("health", "maximum", "shield"):
                start = self.objects[self.own_name] + l[field]
                taken.append((start, start + 4))
        return {"damage_dealt": dealt, "damage_taken": taken, "worm": list(self.worm_ranges)}
