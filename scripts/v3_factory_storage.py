"""Проверка EIP-1186 и полного реестра стандартной фабрики V3 по корню Ethereum Merkle Patricia trie."""
import rlp
from eth_utils import keccak

def word(val):return val.to_bytes(32,"big")
def nibbles(buf):return [digit for b in buf for digit in (b>>4,b&15)]
def hexpath(path,leaf):
    digits=[int(leaf)*2+len(path)%2]+([] if len(path)%2 else [0])+path
    return bytes((a<<4)|b for a,b in zip(digits[::2],digits[1::2]))
def trie_node(rows):
    if len(rows)==1:return [hexpath(rows[0][0],True), rows[0][1]]
    paths=[p for p,v in rows];common=0
    while all(p and len(p)>common and p[common]==paths[0][common] for p in paths):common+=1
    def ref(child):
        encoded=rlp.encode(child)
        return child if len(encoded)<32 else keccak(encoded)
    if common:return [hexpath(paths[0][:common],False),ref(trie_node([(p[common:],v) for p,v in rows]))]
    slots=[b""]*17
    for i in range(16):
        sub=[(p[1:],v) for p,v in rows if p and p[0]==i]
        if sub:slots[i]=ref(trie_node(sub))
    for p,v in rows:
        if not p:slots[16]=v
    return slots

def prove_account(header,proof,factory):
    path=nibbles(keccak(bytes.fromhex(factory[2:])));ref=bytes.fromhex(header["stateRoot"][2:])
    items={keccak(bytes.fromhex(a[2:])):bytes.fromhex(a[2:]) for a in proof["accountProof"]}
    while True:
        node=rlp.decode(items[ref]) if isinstance(ref,bytes) else ref
        if len(node)==17:
            if not path:value=node[16];break
            ref=node[path.pop(0)]
        elif len(node)==2:
            ds=nibbles(node[0]);part=ds[1:] if ds[0]&1 else ds[2:]
            if path[:len(part)]!=part:raise ValueError("путь accountProof")
            path=path[len(part):]
            if ds[0]&2:
                if path:raise ValueError("неполный accountProof")
                value=node[1];break
            ref=node[1]
        else:raise ValueError("узел trie")
    nonce,balance,storage,code=rlp.decode(value)
    if int.from_bytes(nonce,"big")!=int(proof["nonce"],16) or "0x"+storage.hex()!=proof["storageHash"]:
        raise ValueError("accountProof не подтверждает nonce/storage")
    return storage

def map_slot(k,slot):return int.from_bytes(keccak(word(k)+word(slot)),"big")

def verify_directory(data,factory):
    """Проверяет сохранённые первичные данные без сети, включая отсутствие иных ставок и пулов."""
    if data["factory"] != factory:
        raise ValueError("чужая фабрика в архиве")
    root = prove_account({"stateRoot": data["state_root"]}, {
        "accountProof": data["account_proof"], "nonce": hex(data["nonce"]), "storageHash": data["storage_root"],
    },factory)
    layout = data["layout"]
    slots = {}
    if int(data["owner"], 16):
        slots[layout["owner"]] = int(data["owner"], 16)
    for fee, spacing in data["tiers"].items():
        slots[map_slot(int(fee), layout["feeAmountTickSpacing"])] = spacing
    for market in data["pools"]:
        a, b = (int(token, 16) for token in market["tokens"])
        for token_in, token_out in [(a,b), (b,a)]:
            key = map_slot(market["fee"], map_slot(token_out, map_slot(token_in, layout["getPool"])))
            slots[key] = int(market["pool"], 16)
    rows = [(nibbles(keccak(word(slot))), rlp.encode(value)) for slot, value in slots.items()]
    if keccak(rlp.encode(trie_node(rows))) != root:
        raise ValueError("обнаруженные owner/tiers/pools не покрывают хранилище фабрики")
    if len(slots) != data["storage_slots"] or data["nonce"] != len(data["pools"]) + 1:
        raise ValueError("неполное множество физических слотов или адресов")
