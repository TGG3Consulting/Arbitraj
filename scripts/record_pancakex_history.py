"""Публичные заявки PancakeSwap X и исполненные переводы на канонических блоках."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from record_gladius_fills import FILL, TRANSFER, ZERO, IMPL_SLOT, Reader, addr, canonical, hashed

HOST = "https://x.pancakeswap.com/order-handler/orders"
PCS = "0x88815caf0b12208585832b693841c9da19c91687"
V3 = "0xb75bb4c7aaacfa400758bf024c2ffefe17c9cbe3"
NETS = {
    "ethereum": (1, 26494988, ["https://eth.drpc.org", "https://mainnet.gateway.tenderly.co"],
                 [PCS, "0x99afa599c0d8e5d3c4cfca52bb81e5b2b4fd1661",
                  "0x35db01d1425685789dcc9228d47c7a5c049388d8", V3],
                 ["0x47a4158f43b700b985b8b0c64d198259895afaf0b2968fd3325692e3fb341ce0",
                  "0xb4323c7c8c67755d9ed6fe036d8dd2cb441ad593bd2aba2bff1063886f9737a6"]),
    "base": (8453, 62262756, ["https://mainnet.base.org", "https://base.gateway.tenderly.co"],
             [PCS, "0x6b9906d7106e5890852bf98ef13ba5d8761712b9", V3],
             ["0xd5c4927c90f04853b263f332fecf643cb7718bc14e279e76e83ecb2ac0079e9f"]),
}
FILL_DATA = "0x" + keccak(text="FillData(bytes32,address,uint256,(address,uint256,address)[])").hex()


def get_orders(**params):
    res = requests.get(HOST, params=params, timeout=20)
    res.raise_for_status()
    return res.headers.get("Date"), res.json()["orders"]


def live(chain):
    stamp, orders = get_orders(chainId=chain, orderStatus="OPEN", limit=20)
    fields = ("hash", "status", "type", "chainId", "reactor", "offerer", "deadline", "exclusiveFiller", "input", "outputs")
    # Из действующего публичного предложения сохраняем только отображаемые условия.
    return {"http_date": stamp, "limit": 20, "orders": [{k: o[k] for k in fields} for o in orders]}


def native(tx, block, index, wallet):
    url = f"https://eth.blockscout.com/api/v2/transactions/{tx}/internal-transactions"
    payments, params = [], {}
    while True:
        res = requests.get(url, params=params, timeout=25)
        res.raise_for_status()
        page = res.json()
        for row in page["items"]:
            assert (row["transaction_hash"].lower(),row["block_number"],row["transaction_index"]) == (tx, block, index)
            if row["type"] == "call" and row["success"] and int(row["value"] or 0):
                payments.append({"index": row["index"], "from": row["from"]["hash"].lower(),
                                 "to": row["to"]["hash"].lower(), "amount": row["value"]})
        params = page.get("next_page_params")
        if not params:
            return [x for x in payments if wallet in (x["from"], x["to"])]


def header(block):
    return {k: block[k] for k in ("number", "hash", "parentHash", "timestamp", "stateRoot")}


def inspect(r, chain, reactors, txhash):
    tx = r.rpc("eth_getTransactionByHash", [txhash])
    receipt = r.rpc("eth_getTransactionReceipt", [txhash])
    block = r.rpc("eth_getBlockByNumber", [receipt["blockNumber"], False])
    ix = int(receipt["transactionIndex"], 16)
    assert block["transactions"][ix] == tx["hash"] == txhash
    assert receipt["blockHash"] == tx["blockHash"] == block["hash"] and receipt["status"] == "0x1"
    logs = receipt["logs"]
    fills = [l for l in logs if l["address"].lower() in reactors and l["topics"][0] == FILL]
    assert len(fills) == 1
    f = fills[0]
    _, rows = get_orders(orderHash=f["topics"][1])
    assert len(rows) == 1
    o = rows[0]
    filler, owner = map(addr, f["topics"][2:4])
    assert (o["hash"], o["chainId"], o["status"], o["offerer"].lower(), o["filler"].lower()) == (f["topics"][1], chain, "FILLED", owner, filler)
    assert o["transactionHash"].lower() == txhash and int(o["nonce"]) == int(f["data"], 16)
    assert o["reactor"].lower() == f["address"].lower()
    moves = [{"token": l["address"].lower(), "from": addr(t[1]), "to": addr(t[2]),
              "amount": str(int(l["data"], 16)), "index": int(l["logIndex"], 16)}
             for l in logs if (t := l["topics"])[0] == TRANSFER and len(t) == 3]
    paid = [m for m in moves if m["token"] == o["input"]["token"].lower() and m["from"] == owner and m["to"] == filler]
    assert len(paid) == 1 and int(paid[0]["amount"]) > 0
    assert len(o["outputs"]) == 1 and o["outputs"][0]["recipient"].lower() == owner
    output = o["outputs"][0]["token"].lower()
    native_moves = native(txhash, int(block["number"], 16), ix, owner) if output == ZERO and chain == 1 else []
    received = [m for m in moves if m["token"] == output and m["to"] == owner] if output != ZERO else native_moves
    assert len(received) == 1 and received[0]["to"] == owner and int(received[0]["amount"]) > 0
    data = [l for l in logs if l["address"] == f["address"] and l["topics"][:2] == [FILL_DATA, f["topics"][1]]]
    if data:
        assert len(data) == 1 and addr(data[0]["topics"][2]) == paid[0]["token"]
        inp, outs = decode(["uint256", "(address,uint256,address)[]"], bytes.fromhex(data[0]["data"][2:]))
        assert inp == int(paid[0]["amount"]) and outs == ((output, int(received[0]["amount"]), owner),)
    return {"header": header(block), "order": o, "transaction": {k:tx[k] for k in ("hash","from","to","value","input","nonce","blockHash","transactionIndex")},
            "receipt": receipt, "paid": paid[0], "received": received[0], "native_recipient_calls": native_moves,
            "gas_wei": str(int(receipt["effectiveGasPrice"],16)*int(receipt["gasUsed"],16) + int(receipt.get("l1Fee","0x0"),16))}


def run(net):
    chain, n, urls, reactors, samples = NETS[net]
    r = Reader(tuple(urls))
    h = r.rpc("eth_getBlockByNumber", [hex(n), False])
    at = canonical(h)
    roles = {}
    for address in reactors:
        code = bytes.fromhex(r.rpc("eth_getCode", [address, at])[2:])
        impl = addr(r.rpc("eth_getStorageAt", [address, IMPL_SLOT, at]))
        roles[address] = {"runtime": hashed(code) if code else None, "implementation": impl,
                          "implementation_runtime": hashed(bytes.fromhex(r.rpc("eth_getCode", [impl, at])[2:])) if impl != ZERO else None}
    return {"network": net, "head": header(h), "roles": roles, "open_page": live(chain),
            "history": [inspect(r, chain, reactors, tx) for tx in samples]}


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("network", choices=NETS)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.write_text(json.dumps(run(args.network), separators=(",", ":")))
