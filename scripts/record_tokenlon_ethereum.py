"""Воспроизводит уже включённые Tokenlon PMM платежи на родительском Ethereum-блоке."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode, encode
from eth_utils import keccak

from record_base_slipstream_executor import ROOT, calldata
from record_carbon_ethereum import WETH

DATA = ROOT / "tests/data/first_run_dex_2026-11-26/tokenlon_ethereum.json"
ENDPOINTS = ("https://eth.drpc.org", "https://ethereum-rpc.publicnode.com", "https://ethereum.gateway.tenderly.co")
READS = {"eth_chainId", "eth_getTransactionReceipt", "eth_getTransactionByHash", "eth_getBlockByHash",
         "eth_getCode", "eth_getStorageAt", "eth_simulateV1"}
MULTICALL = "0xca11bde05977b3631167028862be2a173976ca11"
EVENT = "0x" + keccak(text="FillOrder(string,bytes32,bytes32,address,address,uint256,address,address,uint256,address,uint256,uint16)").hex()
FILL = ["string","address","uint256","address","address","uint256","address","uint256","uint16"]
ORDER = "(address,address,address,address,uint256,uint256,uint256,uint256,uint256,uint256,bytes,bytes)"
SLOT = hex(int.from_bytes(keccak(text="eip1967.proxy.implementation"), "big") - 1)


def rpc(method, params):
    if method not in READS:
        raise ValueError(method)
    errors = []
    for endpoint in ENDPOINTS:
        try:
            reply = requests.post(endpoint, json=dict(jsonrpc="2.0", id=1, method=method, params=params), timeout=20)
            reply.raise_for_status()
            result = reply.json()
            if "result" in result and result["result"] is not None:
                return result["result"]
            errors.append((endpoint, result.get("error", {}).get("code")))
        except (requests.RequestException, ValueError) as error:
            errors.append((endpoint, type(error).__name__))
    raise RuntimeError((method, errors))


def original(c, roles):
    tx = rpc("eth_getTransactionByHash", [c["hash"]])
    receipt = rpc("eth_getTransactionReceipt", [c["hash"]])
    header = rpc("eth_getBlockByHash", [c["block_hash"], False])
    idx = int(receipt["transactionIndex"], 16)
    assert tx["hash"] == receipt["transactionHash"] == header["transactions"][idx] == c["hash"]
    assert receipt["blockHash"] == tx["blockHash"] == header["hash"] == c["block_hash"]
    assert int(header["number"], 16) == c["block"] and receipt["status"] == "0x1"
    assert tx["to"].lower() == roles["user_proxy"]["address"]
    raw = bytes.fromhex(tx["input"][2:])
    def unpack(data, sig, args):
        assert data[:4] == keccak(text=sig)[:4]
        values = decode(args, data[4:])
        assert encode(args, values) == data[4:]
        return values
    payload, = unpack(raw, "toPMM(bytes)", ["bytes"])
    _, order_data, user_sig = unpack(payload, "fill(uint256,bytes,bytes)", ["uint256","bytes","bytes"])
    sig = "fillOrKillOrder(" + ORDER + ",uint256,bytes)"
    order, taker_qty, maker_sig = unpack(order_data, sig, [ORDER,"uint256","bytes"])
    maker, taker, user = order[:3]
    assert user == c["user"] and maker == c["market_maker"] and taker == roles["pmm"]["address"]
    assert taker_qty == order[5] == int(c["taker"]) and order[4] == int(c["maker"])
    assert int(header["timestamp"], 16) <= order[8] and order[9] % 65536 == c["fee_bps"]
    assert len(user_sig) in (85,86) and user_sig[65:85].hex() == c["recipient"][2:] and maker_sig
    assets = [bytes.fromhex("f47261b0") + encode(["address"], [a]) for a in (c["dst"],c["src"])]
    assert list(order[-2:]) == assets
    expected = ("0x v2", c["src"], taker_qty, maker, c["dst"], order[4], c["recipient"], int(c["net"]), c["fee_bps"])
    assert expected[-2] == order[4] * (10000 - c["fee_bps"]) // 10000
    def event(logs):
        found = [x for x in logs if x["address"].lower() == taker and x["topics"][:1] == [EVENT]]
        assert len(found) == 1
        log = found[0]
        assert log["topics"][2] == c["order_hash"] and log["topics"][3][-40:] == user[2:]
        assert tuple(decode(FILL, bytes.fromhex(log["data"][2:]))) == expected
        return log
    assert int(event(receipt["logs"])["logIndex"], 16) == c["log_index"]
    return tx, receipt, header, event, {"outer_keccak": "0x"+keccak(raw).hex(),
            "maker_signature_bytes": len(maker_sig), "user_signature_bytes": len(user_sig), "expiry": order[8]}


def simulate(c, roles):
    tx, receipt, header, event, proof = original(c, roles)
    tag = dict(blockHash=header["parentHash"], requireCanonical=True)
    # Версии обеих сторон proxy, выбранного PMM и обоих подписанных активов на исходном родителе.
    for key in ("user_proxy", "user_proxy_impl", "pmm"):
        role = roles[key]
        assert "0x"+keccak(hexstr=rpc("eth_getCode", [role["address"], tag])).hex() == role["runtime"]
    proxy = roles["user_proxy"]["address"]
    assert rpc("eth_getStorageAt", [proxy,SLOT,tag])[-40:] == roles["user_proxy_impl"]["address"][2:]
    reads = []
    payer = tx["from"].lower() if c["src"] == WETH.lower() else c["user"]
    for token, who in ((c["src"],payer), (c["dst"],c["recipient"])):
        native = token == WETH.lower()
        reads.append(dict(to=MULTICALL if native else token, data=calldata("getEthBalance(address)" if native else "balanceOf(address)", ["address"],[who])))
    # Исполняем только в симуляции. Исходные ETH и ERC20 остаются на архивных счетах.
    swap = {"from":tx["from"], "to":proxy, "value":tx["value"], "data":tx["input"], "gas":tx["gas"]}
    calls = [*reads,swap,*reads,swap,*reads]
    block = dict(blockOverrides=dict(time=header["timestamp"]),calls=calls)
    # EIP-1898 фиксирует родителя, а time точно воспроизводит истечение заявки.
    child, = rpc("eth_simulateV1", [{"blockStateCalls":[block]},tag])
    assert child["parentHash"] == tag["blockHash"] and child["timestamp"] == header["timestamp"]
    reply = child["calls"]
    assert len(reply) == len(calls)
    base = dict(hash=c["hash"], order_hash=c["order_hash"], source=c["src"], target=c["dst"],
                paid_by=payer, receiver=c["recipient"], tx_sender=tx["from"].lower(), original_gas=int(receipt["gasUsed"],16),
                block=c["block_hash"], parent=header["parentHash"], **proof)
    if reply[2]["status"] != "0x1":
        return {**base,"replay":"original paid; original intra-block prestate unavailable",
                "revert":reply[2].get("error",{}).get("message","")[:140]}
    def amounts(pair):
        assert all(x["status"] == "0x1" and len(x["returnData"]) == 66 for x in pair)
        return [int(x["returnData"],16) for x in pair]
    before, after, retry = [amounts(reply[i:i+2]) for i in (0,3,6)]
    debit, credit = before[0] - after[0], after[1] - before[1]
    assert (debit, credit) == (int(c["taker"]), int(c["net"]))
    event(reply[2]["logs"])
    assert reply[5]["status"] == "0x0" and after == retry
    return {**base,"replay":"paid","paid":str(debit),"received":str(credit),"gas":int(reply[2]["gasUsed"],16),
            "before":list(map(str,before)),"after":list(map(str,after)),"repeat_reverted":True,"logs":reply[2]["logs"]}


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("output",type=Path)
    args = cli.parse_args()
    evidence = json.loads(DATA.read_bytes())
    assert int(rpc("eth_chainId", []),16) == evidence["chain"] == 1
    outcomes = [simulate(c, evidence["roles"]) for c in evidence["controls"]]
    args.output.write_text(json.dumps({"selector":evidence["selector"],"controls":outcomes},indent=2),encoding="utf8")
