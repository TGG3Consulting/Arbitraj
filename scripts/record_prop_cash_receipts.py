"""Reconcile a venue event to both physical ERC20 payments at its canonical Ethereum block."""
from eth_abi import decode
from eth_utils import keccak

from record_prop_cash_sources import CASES

WITNESSES = {
    "kaliber": "0x2736752261a05d8d82aba8f47c2b5b8311412805cb4630968b6156e1e030b6a5",
    "taurus": "0xb409a77781afa1e5cdbe70d62996fa423e7d581c6078190d5c138747bdf58db7",
}
TRANSFER = "0x" + keccak(text="Transfer(address,address,uint256)").hex()
SWAPPED = "0x" + keccak(text="Swapped(address,address,address,uint256,uint256,address)").hex()
IMPLEMENTATION_SLOT = hex(int.from_bytes(keccak(text="eip1967.proxy.implementation"), "big") - 1)


def receipt_evidence(c, name, rpc):
    venue, vault, canonical = CASES[name]
    receipt = rpc(c, "eth_getTransactionReceipt", [WITNESSES[name]])["result"]
    height = int(receipt["blockNumber"], 16)
    selector = {"blockHash": canonical[height], "requireCanonical": True}
    if receipt["blockHash"] != canonical[height] or receipt["status"] != "0x1":
        raise ValueError("unexpected settlement block or failed witness")
    events = [e for e in receipt["logs"] if e["address"].lower() == venue and e["topics"][0] == SWAPPED]
    if len(events) != 1:
        raise ValueError("ambiguous venue witness")
    event = events[0]
    coin_in, coin_out = ["0x" + x[-40:].lower() for x in event["topics"][2:]]
    sold, paid, recipient = decode(["uint256", "uint256", "address"], bytes.fromhex(event["data"][2:]))
    position = int(event["logIndex"], 16)
    transfers = []
    for log in receipt["logs"]:
        if int(log["logIndex"], 16) >= position or log["topics"][0] != TRANSFER or len(log["topics"]) != 3:
            continue
        source, destination = ["0x" + x[-40:].lower() for x in log["topics"][1:]]
        transfers.append((log["address"].lower(), source, destination, int(log["data"], 16), log["logIndex"]))

    def unique(token, source, destination, amount):
        found = [t for t in transfers if t[0] == token and (source is None or t[1] == source)
                 and t[2] == destination and t[3] == amount]
        if len(found) != 1:
            raise ValueError("ambiguous or missing cash movement")
        return found[0]

    entry = unique(coin_in, None, venue, sold)
    custody = unique(coin_in, venue, vault, sold)
    final = unique(coin_out, None, recipient, paid)
    payout = final if final[1] == vault else unique(coin_out, vault, final[1], paid)
    indexes = [int(t[4], 16) for t in (entry, custody, payout)]
    if indexes != sorted(set(indexes)) or indexes[-1] > int(final[4], 16):
        raise ValueError("unordered cash settlement")
    roles = {}
    for role, address in (("venue", venue), ("vault", vault)):
        def codehash(address):
            code = rpc(c, "eth_getCode", [address, selector])["result"]
            if code == "0x":
                raise ValueError("missing code for settlement role")
            return "0x" + keccak(bytes.fromhex(code[2:])).hex()
        roles[role] = {"address": address, "runtime": codehash(address)}
        impl = rpc(c, "eth_getStorageAt", [address, IMPLEMENTATION_SLOT, selector])["result"]
        if int(impl, 16):
            target = "0x" + impl[-40:]
            roles[role].update(implementation=target, implementationHash=codehash(target))
    return {"chainId": 1, "height": height, "hash": receipt["blockHash"], "tx": receipt["transactionHash"],
            "venueLog": event, "payer": entry[1], "recipient": recipient, "spent": sold, "received": paid,
            "transfers": [entry, custody, payout] + ([] if payout == final else [final]), "roles": roles}
