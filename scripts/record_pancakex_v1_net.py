"""Записывает подтверждённую выплату USDT через standalone ExclusiveDutchOrderReactor Ethereum.
Читает опубликованные исполнения и состояние Ethereum. Параметры токена, ERC20-переводы
владелец/исполнитель и Permit2 сверяются по хешам канонических блоков до и после сделки.
Запуск: python scripts/record_pancakex_v1_net.py output.json.gz
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import gzip
import json
from pathlib import Path

import requests
from record_gladius_fills import Reader, TRANSFER, addr, call_data, canonical, hashed
from record_pancakex_history import header, inspect

REACTOR = "0x35db01d1425685789dcc9228d47c7a5c049388d8"
PERMIT = "0x31c2f6fcff4f8759b3bd5bf0e1084a055615c768"
USDT = "0xdac17f958d2ee523a2206206994597c13d831ec7"
WETH = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
RPCS = ("https://ethereum-rpc.publicnode.com", "https://eth.drpc.org", "https://mainnet.gateway.tenderly.co")
SAMPLES = ("0xc2a89988af2f2706b9d4baa71b3843cd287e22cf4b936e32de949c1323b54727",
           "0x3075b0a67962bfa5d6b5fe1ba978ca1630c2fb7d6196d757193aff7506c364b0")


def source(address, at):
    url = "https://sourcify.dev/server/v2/contract/1/" + address
    response = requests.get(url, params={"fields": "compilation,sources,abi,runtimeBytecode.onchainBytecode"}, timeout=20)
    response.raise_for_status()
    src = response.json()
    assert src["address"].lower() == address and str(src["chainId"]) == "1"
    reader = Reader(RPCS)
    code = reader.rpc("eth_getCode", [address, at])
    assert code.lower() == src["runtimeBytecode"]["onchainBytecode"].lower() and len(code) > 2
    return {"address": address, "url": response.url, "match": src["runtimeMatch"],
            "runtime": hashed(bytes.fromhex(code[2:])), "compiledContract": src["compilation"]["fullyQualifiedName"],
            "abi": src["abi"], "sourceHashes": {k: hashed(v["content"].encode()) for k, v in src["sources"].items()}}


def reads(block, specs):
    at = canonical(block)
    def one(spec):
        name, address, sig, types, values = spec
        params = [{"to": address, "data": call_data(sig, types, values)}, at]
        return name, {"method": "eth_call", "params": params, "result": Reader(RPCS).rpc("eth_call", params)}
    with ThreadPoolExecutor(4) as pool:
        return dict(pool.map(one, specs))


def snapshot(block, owner, filler, asset, nonce):
    specs = [(s, USDT, s+"()", [], []) for s in ("basisPointsRate", "maximumFee", "deprecated", "paused")]
    specs += [(s, REACTOR, s+"()", [], []) for s in ("permit2", "feeController")]
    for name, target in (("owner", owner), ("filler", filler), ("reactor", REACTOR)):
        specs.append(("blacklisted_"+name, USDT, "isBlackListed(address)", ["address"], [target]))
    for name, token, holder, spender in (("usdtAllowance", USDT, filler, REACTOR),
                                          ("inputAllowance", asset, owner, PERMIT)):
        specs.append((name, token, "allowance(address,address)", ["address","address"], [holder, spender]))
    for name, token in (("outputBalance", USDT), ("inputBalance", asset)):
        specs.append((name, token, "balanceOf(address)", ["address"], [owner]))
    specs.append(("nonceBitmap", PERMIT, "nonceBitmap(address,uint256)", ["address","uint256"], [owner, nonce >> 8]))
    return reads(block, specs)


def transfer_window(r, block, tokens, owner, txhash):
    topic = "0x"+owner[2:].rjust(64,"0")
    rows = {}
    for topics in ([TRANSFER,topic],[TRANSFER,None,topic]):
        for log in r.rpc("eth_getLogs", [{"address":tokens,"blockHash":block["hash"],"topics":topics}]):
            assert log["blockHash"] == block["hash"] and log["transactionHash"] == txhash and not log["removed"]
            rows[int(log["logIndex"],16)] = log
    return [rows[i] for i in sorted(rows)]


def payment(txhash):
    r = Reader(RPCS)
    paid = inspect(r, 1, [REACTOR], txhash)
    order, block = paid["order"], paid["header"]
    parent = r.rpc("eth_getBlockByHash", [block["parentHash"],False])
    assert r.rpc("eth_getBlockByNumber",[parent["number"],False])["hash"] == parent["hash"]
    owner, filler, asset = order["offerer"].lower(), order["filler"].lower(), order["input"]["token"].lower()
    assert len(order["outputs"]) == 1 and order["outputs"][0]["token"].lower() == USDT
    assert paid["received"]["to"] == owner and paid["received"]["from"] == filler
    nonce = int(order["nonce"])
    before, after = (snapshot(b,owner,filler,asset,nonce) for b in (parent,block))
    val = lambda s, name: int(s[name]["result"],16)
    debit = val(before,"inputBalance") - val(after,"inputBalance")
    net = val(after,"outputBalance") - val(before,"outputBalance")
    logs = transfer_window(r,block,[asset,USDT],owner,txhash)
    assert (debit,net) == (int(paid["paid"]["amount"]),int(paid["received"]["amount"])) and min(debit,net)>0
    for state in (before,after):
        assert addr(state["permit2"]["result"]) == PERMIT
        assert all(val(state,k)==0 for k in ("deprecated","paused","basisPointsRate","maximumFee","blacklisted_filler"))
    mask = 1 << (nonce & 255)
    assert not val(before,"nonceBitmap") & mask and val(after,"nonceBitmap") & mask
    return {**paid,"parent":header(parent),"before":before,"after":after,"ownerBlockTransfers":logs,
            "walletDebit":str(debit),"walletCredit":str(net)}


def run():
    r=Reader(RPCS)
    h=r.rpc("eth_getBlockByNumber",[hex(26494988),False])
    assert h["hash"] == "0x2372196a2961b307333be48d3dad74a2e723fd459c2cb01e225a0a97f50e7a7a"
    roles={name:source(address,canonical(h)) for name,address in (("reactor",REACTOR),("permit2",PERMIT),("usdt",USDT))}
    controls=[payment(h) for h in SAMPLES]
    first=controls[0]["order"]
    assert first["input"]["token"].lower()==WETH
    state=snapshot(h,first["offerer"].lower(),first["filler"].lower(),WETH,int(first["nonce"]))
    return {"referenceBlock":header(h),"deployment":"https://developer.pancakeswap.finance/contracts/unified-swap-api/pcsx-addresses",
            "roles":roles,"referenceState":state,"payments":controls}


if __name__ == "__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    args.output.write_bytes(gzip.compress(json.dumps(run(),separators=(",",":")).encode(),mtime=0))
