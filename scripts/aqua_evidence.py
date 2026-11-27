"""Чтение событий и программ AquaSwapVM v1.0.2.

Схемы взяты из https://github.com/1inch/swap-vm/tree/v1.0.2.
"""
from eth_abi import decode, encode
from eth_abi.exceptions import DecodingError
from eth_utils import keccak

REGISTRY = "0x1111113ccf1426a8e30e2bff5e005d929bf6a90a"
EARLY = "0x499943e74fb0ce105688beee8ef2abec5d936d31"
ROUTER = "0x111111338c5091e8440b67b168bae16a668ac0de"
ORDER = "(address,uint256,bytes)"
INPUTS = [ORDER, "address", "address", "uint256", "bytes"]
EVENTS = {
    "Swapped": ["bytes32", "address", "address", "address", "address", "uint256", "uint256"],
    "Shipped": ["address", "address", "bytes32", "bytes"],
    "Docked": ["address", "address", "bytes32"],
    "Pushed": ["address", "address", "bytes32", "address", "uint256"],
    "Pulled": ["address", "address", "bytes32", "address", "uint256"],
}


def signature(name, types):
    return keccak(text=name + "(" + ",".join(types) + ")")


TOPICS = {"0x" + signature(name, types).hex(): name for name, types in EVENTS.items()}
SWAP = signature("swap", INPUTS)[:4]


def unpack(log):
    name = TOPICS.get(log["topics"][0])
    if not name:
        return None, []
    values = decode(EVENTS[name], bytes.fromhex(log["data"][2:]))
    return name, ["0x" + v.hex() if isinstance(v, bytes) else v for v in values]


def order_record(order):
    maker, traits, data = order
    if not traits & (1 << 254):
        return None
    order_hash = "0x" + keccak(encode([ORDER], [order])).hex()
    program = data[(traits >> (160 + 3 * 16)) & 65535:]
    pos = 0
    instructions = []
    while pos < len(program):
        if pos + 2 > len(program):
            return None
        op, length = program[pos:pos + 2]
        args = program[pos + 2:pos + 2 + length]
        if len(args) != length:
            return None
        instructions.append([op, "0x" + args.hex()])
        pos += 2 + length
    return order_hash, dict(maker=maker, traits=str(traits), data="0x" + data.hex(), instructions=instructions)


def from_transaction(data, expected):
    result = {}
    wire = bytes.fromhex(data[2:])
    start = 0
    while (index := wire.find(SWAP, start)) >= 0:
        start = index + 1
        try:
            args = decode(INPUTS, wire[index + 4:])
            order = order_record(args[0])
        except (DecodingError, ValueError, OverflowError):
            continue
        if order and order[0] in expected:
            result[order[0]] = order[1]
    return result


def call(name, types, values):
    return "0x" + (signature(name, types)[:4] + encode(types, values)).hex()
