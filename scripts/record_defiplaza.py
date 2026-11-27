"""Обычный обмен со своего кошелька на двух опубликованных Ethereum-контрактах DefiPlaza."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak

from fork_fee_rpc import Reader, URLS
from record_base_slipstream_executor import OWNER, ROOT, simulate_block
from record_bitswap_ethereum import tx
from record_ethereum_v2_factories import FUNDING, WETH
from record_llama_base_cl import CanonicalCalls
from record_mooniswap import DAI, ZERO, aggregate, read_balance

USDC = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
USDT = "0xdac17f958d2ee523a2206206994597c13d831ec7"
MARKETS = {
    "multi": ("0xe68c1d72340aeefe5be76eda63ae2f4bc7514110", 13346613,
              "https://github.com/OmegaSyndicate/defiplaza-subgraph/blob/e443f996ea6fb785ec61b1c5762133f01b36f894/subgraph.yaml"),
    "stable": ("0x3a2b8cc91af8bf45f3ec61e779ee1c2ba6b7e694", 15075190,
               "https://github.com/OmegaSyndicate/defiplaza-website/blob/cf3f4804a31bddd80ee35d0d9e9be901a38a7368/src/_stats/pools/stableplaza.njk"),
}


def directory(reader, cap, kind, pool, first):
    if kind == "multi":
        url = "https://sourcify.dev/server/v2/contract/1/" + pool
        response = requests.get(url, params={"fields":"creationBytecode.transformationValues,deployment"}, timeout=30)
        response.raise_for_status()
        proof = response.json()
        args = proof["creationBytecode"]["transformationValues"]["constructorArguments"]
        original = reader.one("eth_getTransactionByHash", [proof["deployment"]["transactionHash"]])
        receipt = reader.one("eth_getTransactionReceipt", [original["hash"]])
        if not original["input"].endswith(args[2:]) or receipt["contractAddress"].lower() != pool or int(receipt["blockNumber"], 16) != first:
            raise ValueError("constructor identity")
        tokens = set(decode(["address[]", "uint256", "string", "string"], bytes.fromhex(args[2:]))[0])
        assert len(tokens) == 15
        logs = reader.one("eth_getLogs", [{"address":pool, "fromBlock":hex(first), "toBlock":reader.header["number"],
                                          "topics":["0x"+keccak(text="BootstrapCompleted(address,address)").hex()]}])
        for log in sorted(logs, key=lambda l: (int(l["blockNumber"], 16), int(l["logIndex"], 16))):
            assert not log.get("removed") and log["address"].lower() == pool
            old, new = decode(["address","address"], bytes.fromhex(log["data"][2:]))
            tokens.remove(old)
            tokens.add(new)
        current = list(cap.one(pool, "listingUpdate()", returns=["address","address"]))
        sources = sorted(tokens | (set(current)-{ZERO}))
        states = [decode(["uint8","uint112"], raw)[0] for raw in aggregate(cap, [
            (pool,"listedTokens(address)",["address"],[asset]) for asset in sources])]
        assets = [ZERO, *(asset for asset, state in zip(sources, states, strict=True) if state == 3)]
        config = list(cap.one(pool, "DFPconfig()", returns=["bool","uint64","uint64"]))
        history = dict(creation=original["hash"], changes=logs, pending=current, states=dict(zip(sources, states)))
    else:
        values = [decode(["address","uint64"],raw) for raw in aggregate(cap, [
            (pool, "tokens(uint256)", ["uint256"], [idx]) for idx in range(4)])]
        assets = [a for a, _ in values]
        indices = [decode(["uint256"], raw)[0] for raw in aggregate(cap, [(pool,"getIndex(address)",["address"],[asset]) for asset in assets])]
        assert indices == list(range(4)) and len(set(assets)) == 4
        config = list(cap.one(pool, "SPconfig()", returns=["uint16","uint8","uint8","uint8","uint8","uint16","uint64","uint64","uint64"]))
        reserves = [decode(["uint64"],raw)[0] for raw in aggregate(cap, [(pool,"reserves(uint256)",["uint256"],[idx]) for idx in indices])]
        history = dict(denorm=[v for _,v in values], reserves=reserves)
    balances = {asset:str(int(reader.one("eth_getBalance",[pool,reader.tag]),16) if asset == ZERO else
                          cap.one(asset,"balanceOf(address)",["address"],[pool],["uint256"])[0]) for asset in assets}
    return dict(config=config, assets=assets, balances=balances, registration=history)


def payment(reader, kind, pool, inventory, src, dst, raw):
    assets = inventory["assets"]
    a, b = assets.index(src), assets.index(dst)
    setup = []
    if src != ZERO:
        deadline = int(reader.header["timestamp"],16)+300
        setup += [tx(FUNDING, "swapETHForExactTokens(uint256,address[],address,uint256)",
                     ["uint256","address[]","address","uint256"], [raw,[WETH,src],OWNER,deadline], value=10**18),
                  tx(src,"approve(address,uint256)",["address","uint256"],[pool,raw])]
    probes = [read_balance(asset, who) for who in (OWNER,pool) for asset in (src,dst)]
    if kind == "stable":
        expected = CanonicalCalls(reader).one(pool,"getOutFromIn(uint256,uint256,uint256)",["uint256"]*3,[a,b,raw],["uint256"])[0]
        event = "Swap(address,address,address,uint256,uint256,address)"
    else:
        x, y = [int(inventory["balances"][asset]) for asset in (src,dst)]
        adjusted = raw*inventory["config"][1]
        expected = adjusted*y//((x<<64)+adjusted)
        event = "Swapped(address,address,address,uint256,uint256)"
    def attempt(limit):
        if kind == "multi":
            action = tx(pool, "swap(address,address,uint256,uint256)",["address","address","uint256","uint256"],
                        [src,dst,raw,limit], value=raw if src == ZERO else 0)
        else:
            action = tx(pool, "easySwap(uint256,uint256,uint256,address)",["uint256"]*3+["address"],[a+(b<<8),raw,limit,OWNER])
        rows = simulate_block(None, reader.tag, {OWNER:{"balance":hex(4*10**18)}},setup+probes+[action]+probes,
                              read_rpc=lambda _,method,params:reader.one(method,params))
        s = len(setup)
        if any(v["status"] != "0x1" for v in rows[:s]):
            return dict(funding_refusal=True)
        assert all(v["status"] == "0x1" for v in rows[s:s+4]+rows[s+5:])
        before,after = ([int(v["returnData"],16) for v in group] for group in (rows[s:s+4],rows[s+5:]))
        op = rows[s+4]
        result = dict(limit=str(limit),status=op["status"],before=list(map(str,before)),after=list(map(str,after)),
                      gas=int(op["gasUsed"],16), approval_gas=int(rows[s-1]["gasUsed"],16) if s else 0)
        if op["status"] == "0x0":
            assert before == after
            result["error"] = op.get("error")
        else:
            credit = after[1]-before[1]
            debit = before[0]-after[0]
            assert (debit,credit) == (raw,expected) == (after[2]-before[2],before[3]-after[3])
            assert int(op["returnData"],16) == credit and credit > 0
            cash = [l for l in op["logs"] if l["address"].lower()==pool and l["topics"]==["0x"+keccak(text=event).hex()]]
            assert len(cash)==1
            types = ["address"]*3+["uint256"]*2+(["address"] if kind=="stable" else [])
            assert decode(types,bytes.fromhex(cash[0]["data"][2:])) == (OWNER.lower(),src,dst,debit,credit)+((OWNER.lower(),) if kind=="stable" else ())
            result.update(spent=str(debit),received=str(credit),logs=op["logs"],action=action)
        return result
    # MultiPlaza checks output > limit and StablePlaza checks output >= limit.
    exact_limit = max(0, expected - int(kind=="multi"))
    exact = attempt(exact_limit)
    high = attempt(exact_limit+1) if exact.get("status") == "0x1" else None
    if high: assert high.get("status") == "0x0"
    return dict(input=src,output=dst,raw=str(raw),expected=str(expected),exact=exact,plus_one=high)


def capture(kind):
    pool, first, source = MARKETS[kind]
    head=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    URLS["ethereum"] = ["https://mainnet.gateway.tenderly.co", *URLS["ethereum"]]
    reader=Reader("ethereum",head)
    assert reader.one("eth_getCode",[OWNER,reader.tag])=="0x"
    state=directory(reader,CanonicalCalls(reader),kind,pool,first)
    pairs=[(ZERO,USDT),(USDC,USDT)] if kind=="multi" else [(USDC,t) for t in state["assets"] if t!=USDC]
    trials=[]
    for x,y in pairs:
        for a,b in ((x,y),(y,x)):
            decimals=18 if a==ZERO else CanonicalCalls(reader).one(a,"decimals()",returns=["uint8"])[0]
            for n in (1,10):
                size=n*10**(15 if a==ZERO else decimals)
                trials.append(payment(reader,kind,pool,state,a,b,size))
    assert reader.one("eth_getBlockByNumber",[reader.header["number"],False])["hash"]==reader.tag["blockHash"]
    code=reader.one("eth_getCode",[pool,reader.tag])
    return dict(kind=kind,chain=1,block=head["block"],selector=reader.tag,source=source,pool=pool,
                runtime="0x"+keccak(hexstr=code).hex(),inventory=state,trials=trials)


if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__)
    cli.add_argument("kind", choices=MARKETS)
    cli.add_argument("output", type=Path)
    opt=cli.parse_args()
    opt.output.write_text(json.dumps(capture(opt.kind),separators=(",",":")),encoding="utf8")

