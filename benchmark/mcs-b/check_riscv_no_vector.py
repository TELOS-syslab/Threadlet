#!/usr/bin/env python3
"""Reject RISC-V vector instructions in executable ELF sections."""

from pathlib import Path
import struct
import sys


EM_RISCV = 243
SHT_PROGBITS = 1
SHF_EXECINSTR = 0x4
OP_V = 0x57
LOAD_FP = 0x07
STORE_FP = 0x27
SYSTEM = 0x73
VECTOR_MEMORY_WIDTHS = {0, 5, 6, 7}
VECTOR_CSRS = {0x008, 0x009, 0x00A, 0x00F, 0xC20, 0xC21, 0xC22}


class ElfError(ValueError):
    pass


def vector_opcode_kind(word):
    opcode = word & 0x7F
    funct3 = (word >> 12) & 0x7
    if opcode == OP_V:
        return "OP-V"
    if opcode in {LOAD_FP, STORE_FP} and funct3 in VECTOR_MEMORY_WIDTHS:
        return "vector load/store"
    if opcode == SYSTEM and funct3 != 0:
        csr = (word >> 20) & 0xFFF
        if csr in VECTOR_CSRS:
            return "vector CSR"
    return None


def find_vector_opcode(code):
    offset = 0
    while offset < len(code):
        if len(code) - offset < 2:
            raise ElfError("truncated instruction")
        halfword = struct.unpack_from("<H", code, offset)[0]
        if halfword & 0x3 != 0x3:
            offset += 2
            continue
        if len(code) - offset < 4:
            raise ElfError("truncated 32-bit instruction")
        if (halfword >> 2) & 0x7 == 0x7:
            raise ElfError("instruction longer than 32 bits")
        word = struct.unpack_from("<I", code, offset)[0]
        kind = vector_opcode_kind(word)
        if kind is not None:
            return offset, word, kind
        offset += 4
    return None


def section_header(data, table, size, index):
    offset = table + size * index
    if size < 64 or offset + 64 > len(data):
        raise ElfError("invalid section header table")
    return struct.unpack_from("<IIQQQQIIQQ", data, offset)


def section_name(names, offset):
    if offset >= len(names):
        raise ElfError("invalid section name")
    end = names.find(b"\0", offset)
    if end < 0:
        raise ElfError("unterminated section name")
    return names[offset:end].decode("ascii", errors="replace")


def executable_sections(data):
    if len(data) < 64 or data[:4] != b"\x7fELF":
        raise ElfError("not an ELF file")
    if data[4] != 2 or data[5] != 1:
        raise ElfError("expected a little-endian ELF64 file")
    if struct.unpack_from("<H", data, 18)[0] != EM_RISCV:
        raise ElfError("expected a RISC-V ELF file")

    table = struct.unpack_from("<Q", data, 40)[0]
    entry_size = struct.unpack_from("<H", data, 58)[0]
    count = struct.unpack_from("<H", data, 60)[0]
    names_index = struct.unpack_from("<H", data, 62)[0]
    if count == 0 or names_index >= count:
        raise ElfError("missing section headers")

    names_header = section_header(data, table, entry_size, names_index)
    names_offset, names_size = names_header[4], names_header[5]
    if names_offset + names_size > len(data):
        raise ElfError("invalid section name table")
    names = data[names_offset : names_offset + names_size]

    sections = []
    for index in range(1, count):
        header = section_header(data, table, entry_size, index)
        name_offset, section_type, flags = header[0], header[1], header[2]
        address, offset, size = header[3], header[4], header[5]
        if section_type != SHT_PROGBITS or not flags & SHF_EXECINSTR:
            continue
        if offset + size > len(data):
            raise ElfError("invalid executable section")
        sections.append(
            (
                section_name(names, name_offset),
                address,
                data[offset : offset + size],
            )
        )
    if not sections:
        raise ElfError("no executable sections")
    return sections


def main():
    if len(sys.argv) != 2:
        print(f"usage: {Path(sys.argv[0]).name} ELF", file=sys.stderr)
        return 2
    path = Path(sys.argv[1])
    try:
        sections = executable_sections(path.read_bytes())
        for name, address, code in sections:
            match = find_vector_opcode(code)
            if match is not None:
                offset, word, kind = match
                print(
                    f"{path}: unsupported {kind} instruction 0x{word:08x} "
                    f"at {name}+0x{offset:x} (0x{address + offset:x})",
                    file=sys.stderr,
                )
                return 1
    except (OSError, ElfError) as error:
        print(f"{path}: RISC-V ISA check failed: {error}", file=sys.stderr)
        return 2

    print(
        f"{path}: no RISC-V vector opcodes in "
        f"{len(sections)} executable sections"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
