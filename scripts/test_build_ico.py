from __future__ import annotations

import os
import struct
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import build_ico as bi


class IcoPacking(unittest.TestCase):
    def test_header_declares_icon_type_and_count(self):
        data = bi.ico_bytes([(16, b"aaa"), (32, b"bbbb")])
        reserved, kind, count = struct.unpack_from("<HHH", data, 0)
        self.assertEqual((reserved, kind, count), (0, 1, 2))

    def test_entry_offsets_point_at_the_image_data(self):
        first, second = b"aaa", b"bbbb"
        data = bi.ico_bytes([(16, first), (32, second)])
        for index, blob in enumerate((first, second)):
            size, offset = struct.unpack_from("<II", data, 6 + 16 * index + 8)
            self.assertEqual(size, len(blob))
            self.assertEqual(data[offset:offset + size], blob)

    def test_256_is_encoded_as_zero(self):
        data = bi.ico_bytes([(256, b"x")])
        width, height = struct.unpack_from("<BB", data, 6)
        self.assertEqual((width, height), (0, 0))

    def test_entry_declares_32bit_color(self):
        data = bi.ico_bytes([(48, b"x")])
        planes, bit_count = struct.unpack_from("<HH", data, 6 + 4)
        self.assertEqual((planes, bit_count), (1, 32))


class CommittedIcon(unittest.TestCase):
    def test_icon_file_is_a_valid_ico(self):
        data = bi.DEFAULT_ICO.read_bytes()
        reserved, kind, count = struct.unpack_from("<HHH", data, 0)
        self.assertEqual((reserved, kind), (0, 1))
        self.assertEqual(count, len(bi.ICO_SIZES))


if __name__ == "__main__":
    unittest.main()
