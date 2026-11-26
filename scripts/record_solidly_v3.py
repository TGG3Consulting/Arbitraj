"""Solidly V3: реестр, байткод, ABI, quoteSwap, разовое allowance и шестипараметрический swap."""
import hashlib
import sys
from pathlib import Path

import requests
from bs4 import BeautifulSoup
from eth_abi import decode
from eth_utils import keccak

from record_lista_ethereum import FOLDER, ROOT, Capture, EXEC, RUNTIME, STEP, calldata, deployed, read, rpc, write
from record_lista_ethereum import HOLDERS as ETH_HOLDERS, WETH as ETH_WETH
from record_woo_base import HOLDERS as BASE_HOLDERS, WETH as BASE_WETH

DOC = "https://docs.solidly.com/resources/contract-addresses.html"
MIN, MAX = 4295128740, 1461446703485210103287273052203988822378723970341
HOLDERS = {
    "base":{**BASE_HOLDERS,"0xfde4c96c8593536e31f229ea8f37b2ada2699bb2":"0x1ecd4e2abd22301a797a28e0f5c9a9eccac97f55",
            "0x2ae3f1ec7f1f5012cfeab0185bfc7aa3cf0dec22":"0xb4807865a786e9e9e26e6a9610f2078e7fc507fb"},
    "ethereum":{**ETH_HOLDERS,ETH_WETH:"0xf977814e90da44bfa03b6295a0616a897441acec",
                "0xd555498a524612c67f286df0e0a9a64a73a7cdc7":"0x9642b23ed1e01df1092b92641051881a322f5d4e"},
}


def simulation(e,net,p,side,dx,deadline):
    source,target = p["tokens"][side],p["tokens"][1-side]
    sender,pool = HOLDERS[net][source],p["pool"]
    direction = side==0
    limit = MIN if direction else MAX
    args,types = [sender,direction,dx,limit],["address","bool","int256","uint160"]
    query = calldata("quoteSwap(bool,int256,uint160)",types[1:],args[1:])
    quoted = rpc(e,"eth_call",[{"from":sender,"to":pool,"data":query},e["selector"]])
    quote = decode(["int256","int256","uint160","int24","uint128"],bytes.fromhex(quoted[2:]))
    out = -quote[1-side]
    if out <= 0 or not 0 < quote[side] <= dx:
        raise ValueError("некорректные знаки/объём котировки")
    gas = 1600000
    if quote[side] < dx:
        capped = {"to":pool,"from":sender,"data":query,"gas":hex(2**24)}
        try:
            measured = int(rpc(e,"eth_estimateGas",[capped,e["selector"]]),16)
        except ValueError as err:
            if "out of gas" not in str(err):
                raise
            return dict(side=side,dx=str(dx),taken=str(quote[side]),out=str(out),quote=quoted,
                        sender=sender,gas_request=capped,gas_error=str(err))
        gas = min(2**24,measured*2+200000)
    def swap(expected,cutoff):
        return calldata("swap(address,bool,int256,uint160,uint256,uint256)",[*types,"uint256","uint256"],[*args,expected,cutoff])
    approve = lambda who,n:calldata("approve(address,uint256)",["address","uint256"],[who,n])
    allowance = calldata("allowance(address,address)",["address","address"],[sender,pool])
    bal = calldata("balanceOf(address)",["address"],[sender])
    above = swap(out+1,deadline)
    late = swap(out,0)
    ex = calldata("execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
        ["address","uint256","address","uint256","uint256",STEP+"[]"],
        [source,dx,target,out,deadline,[(1,pool,source,target,10000,b"")]])
    entries = [("initial_allowance",source,allowance),("reset",source,approve(pool,0)),("approve",source,approve(pool,dx)),
               ("allow_v5",source,approve(EXEC,dx)),("wrong_callback",EXEC,ex),("quote",pool,query),
               ("in_before",source,bal),("out_before",target,bal),("above",pool,above),("expired",pool,late),
               ("exact",pool,swap(out,deadline)),("in_after",source,bal),("out_after",target,bal),("remaining",source,allowance)]
    expensive = {"quote","above","exact","wrong_callback"}
    calls = [{"from":sender,"to":to,"data":data,"gas":hex(gas if label in expensive else 160000)}
             for label,to,data in entries]
    word = lambda x:"0x"+x.to_bytes(32).hex()
    weth = BASE_WETH if net=="base" else ETH_WETH
    runtime = "0x"+RUNTIME.read_text().strip()
    override = {EXEC:{"code":runtime,"stateDiff":{word(0):word(int(sender,16)),word(1):word(int(weth,16))}}}
    block, = rpc(e,"eth_simulateV1",[{"blockStateCalls":[{"calls":calls,"stateOverrides":override}],"validation":False},e["selector"]])
    if block["parentHash"]!=e["selector"]["blockHash"]:
        raise ValueError("другой блок")
    results = {label:r for (label,_,_),r in zip(entries,block["calls"],strict=True)}
    for label,r in results.items():
        if r["status"]!=("0x0" if label in ("above","expired","wrong_callback") else "0x1"):
            raise ValueError(f"{net} {pool} {side} {dx} {label} {r.get('error')}")
    n = lambda label:int(results[label]["returnData"],16)
    paid = decode(["int256","int256"],bytes.fromhex(results["exact"]["returnData"][2:]))
    if (paid!=quote[:2] or results["quote"]["returnData"]!=quoted or n("in_before")-n("in_after")!=quote[side]
            or n("out_after")-n("out_before")!=out or n("remaining")!=dx-quote[side]):
        raise ValueError("котировка расходится с исполнением или фактическим списанием")
    return dict(side=side,dx=str(dx),taken=str(quote[side]),out=str(out),quote=quoted,quote_after=[str(v) for v in quote[2:]],
                sender=sender,calls=calls,results=results)


def main(net,snapshots):
    key = "solidly-v3-base" if net=="base" else "solidly-v3"
    e = read(FOLDER/(net+".json"))
    c = Capture(e)
    response = requests.get(DOC,timeout=18)
    response.raise_for_status()
    doc = BeautifulSoup(response.text,"html.parser")
    domain = "https://basescan.org/" if net=="base" else "https://etherscan.io/"
    addr, = [a.get_text(strip=True).lower() for a in doc.select("a[href]")
             if a["href"].startswith(domain+"address/") and a.previous_sibling
             and "SolidlyV3Factory:" in str(a.previous_sibling)]
    factory,fu = deployed(e,addr)
    if factory["compilation"]["name"]!="SolidlyV3Factory":
        raise ValueError("не распознана фабрика")
    sources = snapshots/(net+"-integration-2026-11-26")/"sources"
    refs = read(sources/"sample-pools.json")[key]["samples"]
    markets,source_info = [],{}
    head = rpc(e,"eth_getBlockByHash",[e["selector"]["blockHash"],False])
    for row in refs:
        pool = row["address"].lower()
        obj,u = deployed(e,pool)
        abi = obj["compilation"]["name"]
        if abi!="SolidlyV3Pool":
            raise ValueError("пул имеет другой ABI")
        contract = obj["compilation"]["fullyQualifiedName"].split(":")[0]
        src = obj["sources"][contract]["content"]
        if (src != factory["sources"][contract]["content"] or "solidlyV3SwapCallback(" not in src
                or "TransferHelper.safeTransferFrom(token0, msg.sender, address(this), uint256(amount0))" not in src):
            raise ValueError("фабрика содержит другую реализацию пула")
        for path,data in obj["sources"].items():
            if factory["sources"][path]["content"]!=data["content"]:
                raise ValueError("расходятся зависимости кодов")
        rpc_abi = requests.get(u,params={"fields":"abi"},timeout=18).json()["abi"]
        sigs = {x["name"]+"("+",".join(a["type"] for a in x["inputs"])+")":x.get("outputs",[]) for x in rpc_abi if x["type"]=="function"}
        sig = "swap(address,bool,int256,uint160,uint256,uint256)"
        if [r["type"] for r in sigs[sig]]!=["int256","int256"]:
            raise ValueError("не тот ABI прямого обмена")
        tokens = [c.one(pool,key+"()",returns=["address"])[0] for key in ("token0","token1")]
        spacing, = c.one(pool,"tickSpacing()",returns=["int24"])
        if (c.one(pool,"factory()",returns=["address"])[0]!=addr or
                c.one(addr,"getPool(address,address,int24)",["address","address","int24"],[*tokens,spacing],["address"])[0]!=pool):
            raise ValueError("пул не зарегистрирован")
        price,tick,fee,unlocked = c.one(pool,"slot0()",returns=["uint160","int24","uint24","bool"])
        liquidity, = c.one(pool,"liquidity()",returns=["uint128"])
        center = (tick//spacing)//256
        ticks = []
        for word in range(center-1,center+2):
            bitmap, = c.one(pool,"tickBitmap(int16)",["int16"],[word],["uint256"])
            for bit in range(256):
                if bitmap & (1<<bit):
                    index = (word*256+bit)*spacing
                    gross,net_liq,on = c.one(pool,"ticks(int24)",["int24"],[index],["uint128","int128","bool"])
                    if not on or not gross:
                        raise ValueError("карта тиков расходится с битмапом")
                    ticks.append([index,str(net_liq),str(gross)])
        market = dict(pool=pool,source=u,tokens=tokens,spacing=spacing,fee=fee,tick=tick,price=str(price),
                      liquidity=str(liquidity),unlocked=unlocked,words=[center-1,center+1],ticks=ticks,cases=[])
        if all(t in HOLDERS[net] for t in tokens):
            for side,t in enumerate(tokens):
                if rpc(e,"eth_getCode",[HOLDERS[net][t],e["selector"]])!="0x":
                    raise ValueError("отправитель не является EOA")
                decimals, = c.one(t,"decimals()",returns=["uint8"])
                sizes = (10**14,10**15) if decimals==18 else (10**6,10**7)
                if net=="ethereum" and pool=="0x831bf48183b999fde45294b14b55199072f0801b" and side==1:
                    sizes = (*sizes,10**18)
                for dx in sizes:
                    market["cases"].append(simulation(e,net,market,side,dx,int(head["timestamp"],16)+300))
        source_info[pool] = dict(digest=hashlib.sha256(src.encode()).hexdigest(),files=len(obj["sources"]),abi={
            name:sigs[name] for name in [sig,"quoteSwap(bool,int256,uint160)","slot0()","ticks(int24)"]})
        markets.append(market)
    write(FOLDER/("solidly_v3_"+net+".json"),dict(chain_id=e["chain_id"],selector=e["selector"],endpoint=e["endpoint"],
          documentation=DOC,factory=addr,factory_verified=fu,market_sources=source_info,markets=markets,
          code={address:e["contracts"][address] for address in [addr,*source_info]},
          executor=EXEC,executor_sha256=hashlib.sha256(bytes.fromhex(RUNTIME.read_text().strip())).hexdigest(),calls=c.calls))
    print(net,[(m["pool"],len(m["cases"])) for m in markets])


if __name__=="__main__":
    main(sys.argv[1],Path(sys.argv[2]))
