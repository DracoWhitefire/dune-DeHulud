"""Mapped damage route for build 26333948; independent of application tables."""
import ctypes
import mmap
import struct

from damage_model import DamageModel, LoggedReader


LAYOUT = {
    "arena_size": 0x800000, "allocation_stride": 0x3000,
    "class": 0x10,                    # UObject.ClassPrivate
    "super": 0x40,                    # UStruct.SuperStruct
    "attributes": 0x1400,             # ADuneCharacter.m_DuneCharacterAttributeSet
    "health_pointer": 0x98 + 0x18,    # CurrentHealth + CurrentValuePtr
    "maximum": 0xC0 + 0xC,            # MaxHealth + CurrentValue
    "shield": 0x320 + 0xC,            # CurrentPower + CurrentValue
    "pawn": 0x388,                    # AController.Pawn
    "acknowledged_pawn": 0x3F0,       # APlayerController.AcknowledgedPawn
    "inventory": 0x1048,              # ADunePlayerController.m_InventoryCoordinator
    "inventory_owner": 0x268,         # UInventoryCoordinator.m_OwnerCharacter
    "aim": 0x1E58,                    # ADunePlayerCharacter.m_CachedAimTarget
    "soft_lock": 0x1D80,              # ADunePlayerCharacter.m_SoftLockTarget
    "gworld": 0xC4D3528,              # GWorld
    "world_gi": 0x3C8,                # UWorld.OwningGameInstance
    "attribute_class_slot": 0xC146A88, # UDuneCharacterAttributeSet
    "controller_class_slot": 0xC175948, # ADunePlayerController
    "inventory_class_slot": 0xC1CD1D8, # UInventoryCoordinator
    "chunks_slot": 0xC337DC0,         # GUObjectArray.ObjObjects.Objects
    "count_slot": 0xC337DD4,          # GUObjectArray.ObjObjects.NumElements
    "shift": 16,                     # FChunkedFixedUObjectArray
    "item_stride": 24,               # FUObjectItem
    "item_object": 0,                # FUObjectItem.Object
    "item_flags": 8,                 # FUObjectItem.Flags
    "item_serial": 16,               # FUObjectItem.SerialNumber
    "weak_reject_mask": 0x30200000,   # FUObjectItem.Flags
    "capacity": 65560,
}


class BoundDamageModel(DamageModel):
    """Share the threat arena's actual controller, character and native classes."""
    def __init__(self, image_base, objects, external_put, external_read, remap_image):
        self.layout = dict(LAYOUT)
        self.memory = mmap.mmap(-1, self.layout["arena_size"])
        self.base = ctypes.addressof(ctypes.c_char.from_buffer(self.memory))
        self.next_offset = 0
        self.external_put, self.external_read = external_put, external_read
        self.remap_image = remap_image
        self.image_base = image_base
        self.worm_objects = objects
        self.objects = {"own": objects["char"], "vehicle": objects["veh"],
                        "controller": objects["pc"], "character_class": objects["c_char"],
                        "vehicle_class": objects["c_veh"], "other_class": objects["c_bogus"]}
        self.handles, self.attributes, self.payloads = {}, {}, {}
        self.worm_ranges = []
        self.own_name, self.image, self.world = "own", 1, 1
        self.worlds = {1: objects["world"]}
        self.reader = LoggedReader(self)
        for name in ("attribute_class", "inventory_class", "inventory", "chunk_table"):
            self.allocate(name)
        size = (1 << self.layout["shift"]) * self.layout["item_stride"]
        self.chunks = [self.allocate("chunk%d" % i, size) for i in range(2)]
        for i, address in enumerate(self.chunks):
            self.put(self.objects["chunk_table"] + i * 8, "Q", address)
        self.character("own", 0, 1)
        self.character("enemy", 1, 1)
        self.character("enemy2", 65537, 1)
        self.character("vehicle", 2, 1, kind="vehicle")
        self.character("other", 3, 1, kind="other")
        self.put(self.objects["inventory"] + self.layout["class"], "Q", self.objects["inventory_class"])
        self.put(objects["pc"] + self.layout["inventory"], "Q", self.objects["inventory"])
        self.seed_slots()
        self.set_pawn("own")
        self.set_handle("aim", "enemy")
        self.set_handle("soft_lock", None)

    def allocate(self, name, size=None):
        size = size or self.layout["allocation_stride"]
        off = (self.next_offset + 0xFFF) & ~0xFFF
        self.next_offset = off + size
        if self.next_offset > len(self.memory):
            raise ValueError("test arena exhausted")
        self.objects[name] = self.base + off
        return self.objects[name]

    def put(self, address, fmt, value):
        if self.base <= address < self.base + len(self.memory):
            super().put(address, fmt, value)
        else:
            self.external_put(address, "<" + fmt, value)

    def read(self, address, size):
        if self.base <= address < self.base + len(self.memory):
            return super().read(address, size)
        return self.external_read(address, size)

    def seed_slots(self):
        for key, name in (("attribute_class_slot", "attribute_class"),
                          ("controller_class_slot", None), ("inventory_class_slot", "inventory_class"),
                          ("chunks_slot", "chunk_table")):
            value = self.worm_objects["c_dune_pc"] if name is None else self.objects[name]
            self.put(self.image_base + self.layout[key], "Q", value)
        self.put(self.image_base + self.layout["count_slot"], "i", self.layout["capacity"])
        self.put(self.image_base + self.layout["gworld"], "Q", self.worlds[self.world])

    def character(self, name, index, serial, health=100, maximum=100, shield=50,
                  kind="character", depth=0, flags=0):
        address = self.objects.get(name) or self.allocate(name)
        cls = self.worm_objects["c_bp_char"] if name == "own" else self.objects[kind + "_class"]
        for level in range(depth):
            child = self.allocate(name + "_class_%d" % level)
            self.put(child + self.layout["super"], "Q", cls)
            cls = child
        self.put(address + self.layout["class"], "Q", cls)
        if name not in self.attributes:
            self.attributes[name] = self.allocate(name + "_attributes")
            self.payloads[name] = self.allocate(name + "_health")
        attrs = self.attributes[name]
        self.put(address + self.layout["attributes"], "Q", attrs)
        self.put(attrs + self.layout["class"], "Q", self.objects["attribute_class"])
        self.put(attrs + self.layout["health_pointer"], "Q", self.payloads[name])
        self.vitals(name, health=health, maximum=maximum, shield=shield)
        self.handles[name] = (index, serial)
        self.table_entry(index, address, serial, flags)
        return address

    def table_entry(self, index, address, serial, flags=0):
        super().table_entry(index, address, serial)
        self.put(self.item(index) + self.layout["item_flags"], "I", flags)

    def vital_address(self, name, field):
        return self.payloads[name] if field == "health" else self.attributes[name] + self.layout[field]

    def vitals(self, name, **values):
        for field, value in values.items():
            self.put(self.vital_address(name, field), "f", float(value))

    def set_pawn(self, name, own="own"):
        self.own_name = own
        self.pawn_name = name
        pawn = self.objects[name] if name else 0
        self.put(self.objects["controller"] + self.layout["pawn"], "Q", pawn)
        self.put(self.objects["controller"] + self.layout["acknowledged_pawn"], "Q", pawn)
        self.put(self.objects["inventory"] + self.layout["inventory_owner"], "Q",
                 self.objects[own] if own else 0)

    def set_handle(self, role, target):
        pair = self.handles[target] if isinstance(target, str) else target
        index, serial = pair if pair is not None else (-1, 0)
        address = self.objects[self.own_name or "own"] + self.layout[role]
        self.put(address, "i", index)
        self.put(address + 4, "i", serial)

    def apply(self, frame):
        # Resolve mutation destinations before installing consumer-side failures.
        previous_image = self.image
        super().apply(dict(frame, fail_reads=[]))
        self.reader.failures = [(self.vital_address(name, field), self.vital_address(name, field) + 4)
                                for name, field in frame.get("fail_reads", [])]
        for entry in frame.get("items", []):
            target = entry["object"]
            self.table_entry(entry["index"], self.objects[target] if target else 0,
                             entry["serial"], entry.get("flags", 0))
        if self.world not in self.worlds:
            address = self.allocate("world_%s" % self.world)
            self.worlds[self.world] = address
            self.put(address + self.layout["class"], "Q", self.worm_objects["c_world"])
            self.put(address + self.layout["world_gi"], "Q", self.worm_objects["gi"])
        if self.image != previous_image:
            self.image_base = self.remap_image()
        self.seed_slots()

    def feature_ranges(self):
        l, stride = self.layout, self.layout["allocation_stride"]
        dealt = [(self.image_base + l["chunks_slot"], self.image_base + l["chunks_slot"] + 8),
                 (self.image_base + l["count_slot"], self.image_base + l["count_slot"] + 4),
                 (self.objects["chunk_table"], self.objects["chunk_table"] + len(self.chunks) * 8)]
        size = (1 << l["shift"]) * l["item_stride"]
        dealt.extend((chunk, chunk + size) for chunk in self.chunks)
        taken = []
        for name in self.handles:
            attrs = self.attributes[name]
            if name == self.own_name:
                taken.extend([(self.objects[name] + l["attributes"], self.objects[name] + l["attributes"] + 8),
                              (attrs + l["class"], attrs + l["class"] + 8),
                              (attrs + l["health_pointer"], attrs + l["health_pointer"] + 8)])
                taken.extend((self.vital_address(name, key), self.vital_address(name, key) + 4)
                             for key in ("health", "maximum", "shield"))
            else:
                dealt.extend((address, address + stride) for address in (attrs, self.payloads[name]))
                # Possessed-pawn class reads also serve the shared owner route.
                if name != self.pawn_name:
                    dealt.append((self.objects[name], self.objects[name] + stride))
                    dealt.extend((address, address + stride) for key, address in self.objects.items()
                                 if key.startswith(name + "_class_"))
        for key in ("aim", "soft_lock"):
            start = self.objects[self.own_name or "own"] + l[key]
            dealt.append((start, start + 8))
        return {"damage_taken": taken, "damage_dealt": dealt, "worm": list(self.worm_ranges)}

    def info(self):
        result = super().info()
        result.update({"bound_build": "26333948", "image_base": self.image_base,
                       "world_address": self.worlds[self.world], "attributes": self.attributes,
                       "vital_addresses": {name: {key: self.vital_address(name, key)
                                                  for key in ("health", "maximum", "shield")}
                                           for name in self.handles}})
        return result
