"""Fake target for the DeHulud tests: a child process that lays out the threat route in its OWN memory.

Run as: python3 fake_game.py DuneSandbox-Win64-Shipping.exe [--timestamp N]
(the exe name is an argument only so that the real /proc/<pid>/cmdline of this child matches).

It prints one JSON line {"base": <image address>, "obj": {name: address}} and then obeys stdin lines:
  f <obj> <offset> <value>   write a float32 at obj+offset (value may be nan / inf / -1)
  i <obj> <offset> <value>   write an int32
  p <obj> <offset> <target>  write a pointer to object <target> (or 0)
  exit                       exit (status 0)
Each command is answered with "ok". Only this process's own memory is ever written.

The constants below are typed in independently of dehulud.py (from the static analysis of build
26083551), so a wrong offset in dehulud.py cannot be hidden by the fake following it.
"""
import ctypes
import json
import mmap
import struct
import sys

SIZE_OF_IMAGE = 220008448          # PE SizeOfImage of build 26083551
TIMESTAMP = 1792504044             # PE TimeDateStamp of build 26083551
GWORLD = 0xC4CF528                 # GWorld slot RVA
SLOTS = {"world": 0xC4CEF00,       # class slot RVAs: UWorld
         "gi": 0xC49DBB0,          # UGameInstance
         "lp": 0xC4AD730,          # ULocalPlayer
         "pc": 0xC4BB0E8,          # APlayerController
         "char": 0xC166430,        # ADuneCharacter
         "pchar": 0xC16F290,       # ADunePlayerCharacter
         "veh": 0xC1E3D68,         # ADuneVehicle
         "comp": 0xC1D8BD8}        # USandwormThreatSourceComponent
CLASS, SUPER = 0x10, 0x40          # UObject.ClassPrivate, UStruct.SuperStruct
WORLD_GI, GI_LP, LP_PC = 0x3C8, 0x38, 0x30          # OwningGameInstance, LocalPlayers (TArray), PlayerController
PC_PAWN, PC_ACK = 0x388, 0x3F0                      # Pawn, AcknowledgedPawn
CHAR_COMP, VEH_COMP = 0xE88, 0xE08                  # m_SandwormThreatSourceComponent (character, vehicle)
BLOB, THRESHOLD = 0x334, 0x348                      # m_CurrentThreatBlobValue, m_CurrentSandwormAttackThreshold


def main(argv):
    stamp = int(argv[argv.index("--timestamp") + 1]) if "--timestamp" in argv else TIMESTAMP
    image = mmap.mmap(-1, SIZE_OF_IMAGE)              # anonymous, lazily backed: only touched pages cost memory
    arena = mmap.mmap(-1, 64 * 0x1000)
    base = ctypes.addressof(ctypes.c_char.from_buffer(image))
    heap = ctypes.addressof(ctypes.c_char.from_buffer(arena))
    names = ["c_world", "c_gi", "c_bp_gi", "c_lp", "c_pc", "c_dune_pc", "c_bp_pc", "c_char", "c_pchar",
             "c_bp_char", "c_veh", "c_wheeled", "c_bp_bike", "c_comp", "c_bogus", "world", "gi", "lp_array",
             "lp", "pc", "char", "char_comp", "veh", "veh_comp", "bogus"]
    obj = {n: heap + 0x1000 * i for i, n in enumerate(names)}

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
                   ("bogus", "c_bogus")):
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
    for comp in ("char_comp", "veh_comp"):            # decoys around the two leaves: an offset error shows
        for off in range(0x300, 0x380, 4):
            put(obj[comp] + off, "<f", 5000.0)
    put(obj["char_comp"] + BLOB, "<f", 42.5)
    put(obj["char_comp"] + THRESHOLD, "<f", 100.0)
    put(obj["veh_comp"] + BLOB, "<f", 80.0)
    put(obj["veh_comp"] + THRESHOLD, "<f", 100.0)

    print(json.dumps({"base": base, "obj": obj}), flush=True)
    for line in sys.stdin:
        cmd = line.split()
        if not cmd or cmd[0] == "exit":
            break
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
