"""Повторная проверка четырёх закреплённых AMM-квитанций, трансферов и развёрнутых V3-фабрик."""
import json
from concurrent.futures import ThreadPoolExecutor

import requests
from record_hashflow_swaps import FILE, TRANSFER

OUT = FILE.with_name("amm_controls.json")
NETWORKS = {
    1: ["https://eth.drpc.org", "https://ethereum-rpc.publicnode.com"],
    56: ["https://bsc-rpc.publicnode.com", "https://bsc-dataseed.bnbchain.org"],
    8453: ["https://base-rpc.publicnode.com", "https://mainnet.base.org"],
    42161: ["https://arbitrum.drpc.org", "https://arb1.arbitrum.io/rpc", "https://arbitrum-one-rpc.publicnode.com"],
}
UNI_SWAP = "0xc42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67"
PANCAKE_SWAP = "0x19b47279256b2a23a1665c810c8d55a1758940ee09377d4f8d26497a3577dc83"


def verify(row):
    chain = row["chain"]

    def read(method, params):
        for url in NETWORKS[chain]:
            try:
                reply = requests.post(url, json={"jsonrpc":"2.0", "id":1, "method":method, "params":params}, timeout=12)
                reply.raise_for_status()
                return reply.json()["result"]
            except (requests.RequestException, KeyError):
                continue
        raise RuntimeError(f"Нет ответа для {chain}, {method}")

    tx = read("eth_getTransactionByHash", [row["tx"]])
    receipt = read("eth_getTransactionReceipt", [row["tx"]])
    block = read("eth_getBlockByNumber", [hex(row["height"]), False])
    assert int(read("eth_chainId", []), 16) == chain
    assert tx["hash"] == receipt["transactionHash"] == row["tx"]
    assert tx["blockHash"] == receipt["blockHash"] == block["hash"] == row["block"]
    assert row["tx"] in block["transactions"] and receipt["status"]=="0x1"
    assert int(tx["nonce"], 16) == row["nonce"] and int(block["timestamp"], 16) == row["timestamp"]
    assert tx["from"].lower() == row["trader"] and tx["to"].lower() == row["router"]
    at = hex(row["height"])
    words = "".join(value[2:].rjust(64, "0") for value in (row["coin"], row["quote"], hex(row["fee"])))
    pool = read("eth_call", [{"to":row["factory"], "data":"0x1698ee82"+words}, at])
    assert "0x"+pool[-40:] == row["pool"]
    for token, decimals in [(row["coin"], row["coin_dec"]), (row["quote"], row["quote_dec"])]:
        assert int(read("eth_call", [{"to":token, "data":"0x313ce567"}, at]),16) == decimals
    topic = PANCAKE_SWAP if chain==56 else UNI_SWAP
    matches = [l for l in receipt["logs"] if l["address"]==row["pool"] and l["topics"][:1]==[topic]]
    assert len(matches) == 1
    who = row["trader"][2:]
    moves = [l for l in receipt["logs"] if len(l["topics"])==3 and l["topics"][0]==TRANSFER
             and l["address"] in (row["coin"],row["quote"])
             and (l["topics"][1][-40:]==who or l["topics"][2][-40:]==who)]
    spends = [l for l in moves if l["topics"][1][-40:]==who]
    gets = [l for l in moves if l["topics"][2][-40:]==who]
    assert len(spends)==len(gets)==1 and gets[0]["address"] != spends[0]["address"]
    assert (row["side"]=="Sell") == (spends[0]["address"]==row["coin"])
    assert int(spends[0]["data"],16)==int(row["amount_in"]) and int(gets[0]["data"],16)==int(row["amount_out"])
    selected = {k:receipt[k] for k in ("status","blockNumber","gasUsed","effectiveGasPrice","transactionIndex","l1Fee","operatorFeeScalar","operatorFeeConstant","daFootprintGasScalar") if k in receipt}
    selected["logs"] = matches + moves
    row["receipt"] = selected


if __name__=="__main__":
    rows = json.loads(OUT.read_text("utf8"))
    with ThreadPoolExecutor(max_workers=4) as pool:
        list(pool.map(verify, rows))
    OUT.write_text(json.dumps(rows,indent=2)+"\n",encoding="utf8")
    print("Verified", [(r["chain"],r["height"]) for r in rows])
