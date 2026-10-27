"""Starts the repository's fake game (tests/fake_game.py) with its image mapped from a FILE named
DuneSandbox-Win64-Shipping.exe at offset 0, the way Wine maps the real game, so that an unmodified DeHulud
finds it through /proc/<pid>/maps like a real game.

Run as: python3 fake_file_image.py DuneSandbox-Win64-Shipping.exe <folder for the image file>
(the exe name is an argument only so that /proc/<pid>/cmdline of this process names it). The sparse image file
is created in <folder>; the mapping is private, so the fake's writes never reach the file.
"""
import ctypes
import mmap
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(HERE)), "tests"))

import fake_game  # noqa: E402

_anonymous = fake_game.region


def region(size, at=None):
    if size != fake_game.SIZE_OF_IMAGE or at is not None:
        return _anonymous(size, at)
    path = os.path.join(sys.argv[2], sys.argv[1])
    fd = os.open(path, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        os.ftruncate(fd, size)
        m = mmap.mmap(fd, size, flags=mmap.MAP_PRIVATE, prot=mmap.PROT_READ | mmap.PROT_WRITE)
    finally:
        os.close(fd)
    return m, ctypes.addressof(ctypes.c_char.from_buffer(m))


fake_game.region = region
fake_game.main([sys.argv[1]])
