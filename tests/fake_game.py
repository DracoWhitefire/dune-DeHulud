"""Fake target for the DeHulud tests: a child process that lays out the threat route in its OWN memory.

Run as: python3 fake_game.py DuneSandbox-Win64-Shipping.exe [--build B] [--timestamp N] [--fixed]
(the exe name is an argument only so that the real /proc/<pid>/cmdline of this child matches).
--fixed maps the image and the object arena at fixed addresses, so that an exec keeps them where they were.

It prints one JSON line {"base": <image address>, "obj": {name: address}, "size": <SizeOfImage>, "timestamp": <TimeDateStamp>,
"build": <id>}
and then obeys stdin lines:
  f <obj> <offset> <value>   write a float32 at obj+offset (value may be nan / inf / -1)
  i <obj> <offset> <value>   write an int32
  p <obj> <offset> <target>  write a pointer to object <target> (or 0)
  exec <timestamp>           exec a new fake game in this process (same pid; with --fixed also the same
                             addresses) whose PE header has the given TimeDateStamp; it prints a new JSON line
  exit                       exit (status 0)
Each other command is answered with "ok". Only this process's own memory is ever written.

The constants below are typed in independently of dehulud.py (from the static analysis of builds 26333948,
26224975 and 26083551), so a wrong offset in dehulud.py cannot be hidden by the fake following it. --build picks
the image layout (default 26083551); the member offsets are the same in all builds.
"""
import ctypes
import json
import mmap
import os
import struct
import sys

IMAGES = {   # build: (PE SizeOfImage, PE TimeDateStamp, GWorld slot RVA, class slot RVAs)
    "26333948": (220024832, 1793384724, 0xC4D3528,
                 {"world": 0xC4D2F00,
                  "gi": 0xC4A1BC0,
                  "lp": 0xC4B1730,
                  "pc": 0xC4BF0E8,
                  "char": 0xC16A450,
                  "pchar": 0xC1732B0,
                  "veh": 0xC1E7D88,
                  "comp": 0xC1DCBF8}),
    "26083551": (220008448, 1792504044, 0xC4CF528,
                 {"world": 0xC4CEF00,       # class slot RVAs: UWorld
                  "gi": 0xC49DBB0,          # UGameInstance
                  "lp": 0xC4AD730,          # ULocalPlayer
                  "pc": 0xC4BB0E8,          # APlayerController
                  "char": 0xC166430,        # ADuneCharacter
                  "pchar": 0xC16F290,       # ADunePlayerCharacter
                  "veh": 0xC1E3D68,         # ADuneVehicle
                  "comp": 0xC1D8BD8}),      # USandwormThreatSourceComponent
    "26224975": (219996160, 1793030166, 0xC4CC568,
                 {"world": 0xC4CBF40,
                  "gi": 0xC49ABF0,
                  "lp": 0xC4AA770,
                  "pc": 0xC4B8128,
                  "char": 0xC163440,
                  "pchar": 0xC16C2A0,
                  "veh": 0xC1E0D78,
                  "comp": 0xC1D5BE8}),
}
BUILD = sys.argv[sys.argv.index("--build") + 1] if "--build" in sys.argv else "26083551"
SIZE_OF_IMAGE, TIMESTAMP, GWORLD, SLOTS = IMAGES[BUILD]
CLASS, SUPER = 0x10, 0x40          # UObject.ClassPrivate, UStruct.SuperStruct
WORLD_GI, GI_LP, LP_PC = 0x3C8, 0x38, 0x30          # OwningGameInstance, LocalPlayers (TArray), PlayerController
PC_PAWN, PC_ACK = 0x388, 0x3F0                      # Pawn, AcknowledgedPawn
CHAR_COMP, VEH_COMP = 0xE88, 0xE08                  # m_SandwormThreatSourceComponent (character, vehicle)
BLOB, THRESHOLD = 0x334, 0x348                      # m_CurrentThreatBlobValue, m_CurrentSandwormAttackThreshold
FIXED_IMAGE, FIXED_ARENA = 0x3000000000, 0x4000000000  # --fixed addresses (far below the usual mmap area)


def region(size, at=None):
    """Anonymous read/write memory, lazily backed (only touched pages cost memory): (buffer, address)."""
    if at is None:
        m = mmap.mmap(-1, size)
        return m, ctypes.addressof(ctypes.c_char.from_buffer(m))
    libc = ctypes.CDLL(None, use_errno=True)
    libc.mmap.restype = ctypes.c_void_p
    libc.mmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_long]
    # PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE (never replaces a mapping)
    got = libc.mmap(at, size, 0x3, 0x22 | 0x100000, -1, 0)
    if got != at:
        raise SystemExit("fake_game: cannot map %#x (errno %d)" % (at, ctypes.get_errno()))
    return (ctypes.c_char * size).from_address(at), at


def main(argv):
    # Optional synthetic damage arena; existing threat addresses and commands stay unchanged.
    from damage_model import DamageModel
    bound_damage = "--damage" in argv and BUILD == "26333948"
    damage = DamageModel() if "--damage" in argv and not bound_damage else None
    stamp = int(argv[argv.index("--timestamp") + 1]) if "--timestamp" in argv else TIMESTAMP
    fixed = "--fixed" in argv
    image, base = region(SIZE_OF_IMAGE, FIXED_IMAGE if fixed else None)
    object_stride = 0x3000 if bound_damage else 0x1000
    arena, heap = region(64 * object_stride, FIXED_ARENA if fixed else None)
    names = ["c_world", "c_gi", "c_bp_gi", "c_lp", "c_pc", "c_dune_pc", "c_bp_pc", "c_char", "c_pchar",
             "c_bp_char", "c_veh", "c_wheeled", "c_bp_bike", "c_comp", "c_bogus", "world", "gi", "lp_array",
             "lp", "pc", "char", "char_comp", "veh", "veh_comp", "bogus", "char2", "char2_comp"]
    obj = {n: heap + object_stride * i for i, n in enumerate(names)}

    def put(addr, fmt, value):
        mem = image if base <= addr < base + SIZE_OF_IMAGE else arena
        struct.pack_into(fmt, mem, addr - (base if mem is image else heap), value)

    # PE32+ header (MZ, e_lfanew 0x80, PE, AMD64, TimeDateStamp, magic 0x20b, SizeOfImage)
    image[0:2] = b"MZ"
    put(base + 0x3C, "<I", 0x80)
    image[0x80:0x84] = b"PE\0\0"
    put(base + 0x84, "<H", 0x8664)
    put(base + 0x88, "<I", stamp)
    put(base + 0x98, "<H", 0x20B)
    put(base + 0x98 + 56, "<I", SIZE_OF_IMAGE)
    # class slots hold the UClass* (native classes); Blueprint classes chain to them through SuperStruct
    for key, cls in (("world", "c_world"), ("gi", "c_gi"), ("lp", "c_lp"), ("pc", "c_pc"), ("char", "c_char"),
                     ("pchar", "c_pchar"), ("veh", "c_veh"), ("comp", "c_comp")):
        put(base + SLOTS[key], "<Q", obj[cls])
    for child, parent in (("c_bp_gi", "c_gi"), ("c_dune_pc", "c_pc"), ("c_bp_pc", "c_dune_pc"),
                          ("c_pchar", "c_char"), ("c_bp_char", "c_pchar"), ("c_wheeled", "c_veh"),
                          ("c_bp_bike", "c_wheeled")):
        put(obj[child] + SUPER, "<Q", obj[parent])
    for o, cls in (("world", "c_world"), ("gi", "c_bp_gi"), ("lp", "c_lp"), ("pc", "c_bp_pc"),
                   ("char", "c_bp_char"), ("char_comp", "c_comp"), ("veh", "c_bp_bike"), ("veh_comp", "c_comp"),
                   ("bogus", "c_bogus"), ("char2", "c_bp_char"), ("char2_comp", "c_comp")):
        put(obj[o] + CLASS, "<Q", obj[cls])
    put(base + GWORLD, "<Q", obj["world"])
    put(obj["world"] + WORLD_GI, "<Q", obj["gi"])
    put(obj["gi"] + GI_LP, "<Q", obj["lp_array"])     # TArray Data
    put(obj["gi"] + GI_LP + 8, "<i", 1)               # TArray Num
    put(obj["gi"] + GI_LP + 12, "<i", 1)              # TArray Max
    put(obj["lp_array"], "<Q", obj["lp"])
    put(obj["lp"] + LP_PC, "<Q", obj["pc"])
    put(obj["pc"] + PC_ACK, "<Q", obj["char"])        # on foot: Pawn == AcknowledgedPawn == the character
    put(obj["pc"] + PC_PAWN, "<Q", obj["char"])
    put(obj["char"] + CHAR_COMP, "<Q", obj["char_comp"])
    put(obj["veh"] + VEH_COMP, "<Q", obj["veh_comp"])
    put(obj["char2"] + CHAR_COMP, "<Q", obj["char2_comp"])   # a second player character (7 %), not possessed
    for comp in ("char_comp", "veh_comp", "char2_comp"):  # decoys around the two leaves: an offset error shows
        for off in range(0x300, 0x380, 4):
            put(obj[comp] + off, "<f", 5000.0)
    put(obj["char_comp"] + BLOB, "<f", 42.5)
    put(obj["char_comp"] + THRESHOLD, "<f", 100.0)
    put(obj["veh_comp"] + BLOB, "<f", 80.0)
    put(obj["veh_comp"] + THRESHOLD, "<f", 100.0)
    put(obj["char2_comp"] + BLOB, "<f", 7.0)
    put(obj["char2_comp"] + THRESHOLD, "<f", 100.0)

    image_mappings = [image]

    def read_external(address, size):
        for memory, start in ((image, base), (arena, heap)):
            if start <= address and address + size <= start + len(memory):
                return bytes(memory[address - start:address - start + size])
        raise ValueError("outside fake game mappings")

    def remap_image():
        nonlocal image, base
        new_image, new_base = region(SIZE_OF_IMAGE)
        new_image[:0x400] = image[:0x400]
        # Invalidate the old PE identity so an attached reader really reattaches.
        struct.pack_into("<I", image, 0x88, 0)
        image, base = new_image, new_base
        image_mappings.append(image)
        for key, cls in (("world", "c_world"), ("gi", "c_gi"), ("lp", "c_lp"), ("pc", "c_pc"),
                         ("char", "c_char"), ("pchar", "c_pchar"), ("veh", "c_veh"), ("comp", "c_comp")):
            put(base + SLOTS[key], "<Q", obj[cls])
        return base

    if bound_damage:
        from damage_build import BoundDamageModel
        damage = BoundDamageModel(base, obj, put, read_external, remap_image)

    if damage is not None:
        damage.worm_ranges = [(obj[name] + field, obj[name] + field + 4)
                              for name in ("char_comp", "veh_comp", "char2_comp")
                              for field in (BLOB, THRESHOLD)]

    info = {"base": base, "obj": obj, "size": SIZE_OF_IMAGE, "timestamp": stamp, "build": BUILD}
    if damage is not None:
        info["damage"] = damage.info()
    print(json.dumps(info), flush=True)
    for line in sys.stdin:
        cmd = line.split()
        if not cmd or cmd[0] == "exit":
            break
        if cmd[0] == "damage":
            frame = json.loads(line.split(" ", 1)[1])
            if damage is None:
                reply = {"error": "damage arena not enabled"}
            elif not bound_damage and ("world" in frame or "image" in frame):
                reply = {"error": "unbound identity: use threat pointer/PE commands until route binding"}
            else:
                damage.apply(frame)
                reply = {"damage": damage.info()}
            print(json.dumps(reply), flush=True)
            continue
        if cmd[0] == "damage-info":
            print(json.dumps(damage.info()), flush=True)
            continue
        if cmd[0] == "exec":                          # another image in this same process
            os.execv(sys.executable, [sys.executable, "-B", os.path.abspath(__file__), argv[0], "--timestamp", cmd[1],
                                      "--build", BUILD] + (["--fixed"] if fixed else []) +
                                      (["--damage"] if damage is not None else []))
        op, where, off, value = cmd[0], obj[cmd[1]], int(cmd[2], 0), cmd[3]
        if op == "f":
            put(where + off, "<f", float(value))
        elif op == "i":
            put(where + off, "<i", int(value, 0))
        elif op == "p":
            put(where + off, "<Q", 0 if value == "0" else obj[value])
        print("ok", flush=True)


if __name__ == "__main__":
    main(sys.argv[1:])
