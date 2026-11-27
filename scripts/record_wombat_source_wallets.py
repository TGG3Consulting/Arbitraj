"""Reconcile DefiLlama #1700 registered spot pools, then measure the missing same-chain routes."""
import argparse
import json
import re
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak
from record_base_slipstream_executor import EXEC, OWNER, ROOT, RUNTIME, STEP, calldata, simulate_block
from record_bitswap_ethereum import balance, tx
from record_concave_ethereum import approval
from record_woo_base import SLOT
from fork_fee_rpc import Reader, URLS

DATA = ROOT / "tests/data/first_run_dex_2026-11-26"
ADAPTER = "https://raw.githubusercontent.com/DefiLlama/dimension-adapters/be9967c3f0d3ccd06c871bab0d16beb3304de719/dexs/wombat-exchange/index.ts"
WOM = "0xc0b314a8c08637685fc3dafc477b92028c540cfb"
MWOM = "0xca9fb45f6600ee6e47e73666a6b7dd407d131f96"
WETH = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
# Historical public token holders fund an ordinary simulator wallet by transfer() only.
# No stored balance or allowance of a token or market is modified.
HOLDERS = {
    "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913": "0x179c94e8cb6e30f2f74e1dd467502aa92d21298e",
    "0xd9aaec86b65d86f6a7b5b1b0c42ffa531710b6ca": "0x5c687d1659bc0029ddda3369e7093d214c611fc3",
    "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48": "0xfe9d105d05d0a769ecc096453486edfe845a585c",
    "0xdac17f958d2ee523a2206206994597c13d831ec7": "0xf977814e90da44bfa03b6295a0616a897441acec",
    "0x2260fac5e5542a773aa44fbcfedf7c193bc2c599": "0x176f3dab24a159341c0509bb36b833e7fdd0a132",
    WOM: "0x51f2c97b999a2a346073245079f0e35b21360d79",
    "0xeff2b1353cdcaa2c3279c2bfde72120c7ffb5e24": "0xc6963e89c59d568f90ae0894547457dd570fc8ca",
}
SWAP = "swap(address,address,uint256,uint256,address,uint256)"
SWAP_TYPES = ["address","address","uint256","uint256","address","uint256"]
QUOTE = "quotePotentialSwap(address,address,int256)"
word = lambda n: "0x"+format(n,"064x")


def source_pools(network):
    response = requests.get(ADAPTER, timeout=20)
    response.raise_for_status()
    stanza = response.text.split("[CHAIN."+network.upper()+"]:",1)[1].split("pools: {",1)[1].split("}",1)[0]
    rows = {name:a.lower() for name,a in re.findall(r'(\w+): "(0x[0-9a-fA-F]{40})"',stanza)}
    assert len(rows) == len(set(rows.values())) == {"base":2,"ethereum":10}[network]
    return rows


def capture(network, url):
    old = json.loads((DATA/f"wombat_{network}.json").read_bytes())
    head = json.loads((ROOT/"tests/data/base_ethereum_networks.json").read_bytes())[network]
    tag = old["selector"]
    URLS[network] = list(dict.fromkeys([*URLS[network],url]))
    reader = Reader(network,{"chain_id":head["chain"],"block":head["block"],"selector":tag})
    request = reader.one
    h = request("eth_getBlockByNumber",[hex(head["block"]),False])
    assert h["hash"] == head["hash"] == tag["blockHash"] and int(request("eth_chainId",[]),16)==head["chain"]
    assert all(request("eth_getCode",[who,tag])=="0x" for who in [EXEC,OWNER])
    wrapped = WETH if network=="ethereum" else "0x4200000000000000000000000000000000000006"
    runtime = "0x"+RUNTIME.read_text().strip().removeprefix("0x")
    overrides = {OWNER:{"balance":hex(10**18)},EXEC:{"code":runtime,"stateDiff":{word(0):word(int(OWNER,16)),word(1):word(int(wrapped,16))}}}
    def sim(calls):
        return simulate_block(url,tag,overrides,calls,lambda _,m,p:request(m,p))
    def one(a,sig,ret,types=(),values=()):
        raw=request("eth_call",[{"to":a,"data":calldata(sig,types,values)},tag])
        return decode([ret],bytes.fromhex(raw[2:]))[0]
    def send_in(token,qty):
        if token==wrapped:
            return [tx(token,"deposit()",value=qty)]
        donor = HOLDERS[token]
        assert request("eth_getCode",[donor,tag])=="0x" and one(token,"balanceOf(address)","uint256",["address"],[donor])>=qty
        return [dict(tx(token,"transfer(address,uint256)",["address","uint256"],[OWNER,qty]),**{"from":donor})]
    expiry = int(h["timestamp"],16)+300
    issued = source_pools(network)
    preset = next(x["settings"] for x in json.loads((ROOT/"config/first_run_dex.json").read_bytes()) if x["network"]==network and x["id"]=="wombat-exchange")
    rows=[]
    for name,pool in issued.items():
        imp="0x"+request("eth_getStorageAt",[pool,SLOT,tag])[-40:]
        poolcode,implcode=[request("eth_getCode",[a,tag]) for a in [pool,imp]]
        assert poolcode!="0x" and implcode!="0x"
        tokens=one(pool,"getTokens()","address[]")
        assets={}
        for t in tokens:
            asset=one(pool,"addressOfAsset(address)","address",["address"],[t])
            assets[t]=dict(asset=asset,decimals=one(t,"decimals()","uint8"),cash=str(one(asset,"cash()","uint256")),
                           liability=str(one(asset,"liability()","uint256")),physical=str(one(t,"balanceOf(address)","uint256",["address"],[asset])))
        row=dict(name=name,pool=pool,implementation=imp,proxy_hash="0x"+keccak(hexstr=poolcode).hex(),implementation_hash="0x"+keccak(hexstr=implcode).hex(),
                 in_prior_document=pool in old["documented"],in_preset=pool in preset["dex.stable_pools"],paused=one(pool,"paused()","bool"),
                 hair_cut=str(one(pool,"haircutRate()","uint256")),assets=assets,legs=[])
        rows.append(row)
        # Keep the same markets after first-run grows, for reproducibility of every strict control.
        if name not in ("crossChainPool","volatilePool","mWOM","wmxWOM","frax"):
            continue
        for x in tokens:
            for y in tokens:
                if x==y:
                    continue
                dec=assets[x]["decimals"]
                physical=int(assets[x]["cash"])*10**dec//10**18
                budgets=sorted({max(1,physical//100_000),max(2,physical//1000)}) if physical else [1,10**max(dec-3,0)]
                for qty in budgets:
                    def pool_swap(a,b,n,minimum,end):
                        return tx(pool,SWAP,SWAP_TYPES,[a,b,n,minimum,OWNER,end])
                    if x==MWOM:
                        # mWOM is acquired through a real WOM purchase. Both methods see the same changed pool.
                        other=3*qty
                        prefix=send_in(WOM,other)+[approval(WOM,pool,other),pool_swap(WOM,MWOM,other,qty,expiry)]
                    elif x in HOLDERS or x==wrapped:
                        prefix=send_in(x,qty)
                    else:
                        # Empty FRAX market: measure both canonical views without inventing FRAX funding.
                        q,=sim([tx(pool,QUOTE,["address","address","int256"],[x,y,qty])])
                        row["legs"].append(dict(source=x,target=y,qty=str(qty),mode="view",quote=q))
                        continue
                    for mode in ("pool","v5"):
                        spender=pool if mode=="pool" else EXEC
                        prepare=prefix+[approval(x,spender,qty)]
                        def trade(minimum,end):
                            if mode=="pool":
                                return pool_swap(x,y,qty,minimum,end)
                            step=(30,pool,x,y,10000,b"")
                            return tx(EXEC,f"execute(address,uint256,address,uint256,uint256,{STEP}[])",
                                      ["address","uint256","address","uint256","uint256",STEP+"[]"],[x,qty,y,minimum,end,[step]])
                        quote=tx(pool,QUOTE,["address","address","int256"],[x,y,qty])
                        checks=[balance(x,OWNER),balance(y,OWNER)]
                        calls=prepare+[quote]+checks+[trade(1,expiry)]+checks
                        res=sim(calls)
                        assert all(r["status"]=="0x1" for r in res[:len(prepare)])
                        q,bx,by,trade_result,ax,ay=res[-6:]
                        assert all(r["status"]=="0x1" for r in (bx,by,ax,ay))
                        paid=int(bx["returnData"],16)-int(ax["returnData"],16)
                        net=int(ay["returnData"],16)-int(by["returnData"],16)
                        ok=trade_result["status"]=="0x1"
                        assert (paid==qty and net>0) if ok else (paid==net==0)
                        leg=dict(source=x,target=y,qty=str(qty),mode=mode,funding=prefix,calls=calls,results=res,quote=q,
                                 paid=str(paid),received=str(net),ok=ok)
                        if ok:
                            controls=[trade(net+1,expiry),trade(net,1),trade(net,expiry)]
                            checked=sim(prepare+controls)[-3:]
                            leg["controls"]=dict(calls=controls,results=checked)
                            assert [r["status"] for r in checked]==["0x0","0x0","0x1"]
                        row["legs"].append(leg)
    assert request("eth_getBlockByNumber",[hex(head["block"]),False])["hash"]==h["hash"]
    return dict(head=head,providers=reader.by_method,source=ADAPTER,owner=OWNER,executor=EXEC,v5_hash="0x"+keccak(hexstr=runtime).hex(),pools=rows)


if __name__=="__main__":
    parser=argparse.ArgumentParser()
    parser.add_argument("network",choices=["base","ethereum"])
    parser.add_argument("output",type=Path)
    arg=parser.parse_args()
    result=capture(arg.network,"https://"+("base" if arg.network=="base" else "mainnet")+".gateway.tenderly.co")
    arg.output.write_text(json.dumps(result),encoding="utf-8")
