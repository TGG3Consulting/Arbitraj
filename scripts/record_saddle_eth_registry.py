"""Проверяет все записи официального Saddle PoolRegistry Ethereum и пулы таблицы проекта на закреплённом блоке."""
import re

from eth_abi import decode
from eth_utils import keccak
import requests

from record_balancer_deployments import ROOT, read, write
from record_balancer_markets import Capture
from record_curve_ng_deployments import code
from record_base_slipstream_executor import calldata
from record_woo_base import rpc

DOC="https://docs.saddle.finance/contracts.md"
FOLDER=ROOT/"tests/data/first_run_dex_2026-11-26"
TUPLE="(address,address,uint8,bytes32,address,address[],address[],address,address,bool,bool,bool)"
REV="2cfdd05580efb626d79d597034b2cf5ae89b5626"
HOLDERS={"0x4fabb145d64652a948d72533023f6e7a623c7c53":"0xe9e7cea3dedca5984780bafc599bd69add087d56",
         "0xdac17f958d2ee523a2206206994597c13d831ec7":"0xf977814e90da44bfa03b6295a0616a897441acec",
         "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48":"0x37305b1cd40574e4c5ce33f8e8306be057fd7341"}


def take(c, calls):
    values=[]
    for i in range(0,len(calls),3):
        values.extend(c.batch(calls[i:i+3]))
    return values


def implementation(e):
    metadata={}
    for name in ("SwapUtils","AmplificationUtils","PermissionlessSwap"):
        url=f"https://raw.githubusercontent.com/saddle-finance/saddle-contract/{REV}/deployments/mainnet/{name}.json"
        response=requests.get(url,timeout=20)
        response.raise_for_status()
        j=response.json()
        address=j["address"].lower()
        actual=rpc(e,"eth_getCode",[address,e["selector"]])
        compiled=j["deployedBytecode"].lower()
        if name!="PermissionlessSwap":
            if not compiled.startswith("0x73"+"00"*20):
                raise ValueError("не распознана защита вызова библиотеки")
            compiled="0x73"+address[2:]+compiled[44:]
        else:
            for lib,addr in j["libraries"].items():
                if addr.lower()!=metadata[lib]["address"]:
                    raise ValueError("адрес библиотеки не совпал")
                placeholder="__$"+keccak(text=f"contracts/{lib}.sol:{lib}").hex()[:34]+"$__"
                compiled=compiled.replace(placeholder,addr[2:].lower())
            sourcify=f"https://sourcify.dev/server/v2/contract/1/{address}?fields=runtimeBytecode.immutableReferences"
            verified=requests.get(sourcify,timeout=20).json()
            immutable,=verified["runtimeBytecode"]["immutableReferences"].values()
            master,=j["args"]
            for item in immutable:
                start=2+2*item["start"]
                if item["length"]!=32 or compiled[start:start+64]!="0"*64:
                    raise ValueError("неизвестная разметка immutable")
                compiled=compiled[:start]+master[2:].lower().zfill(64)+compiled[start+64:]
        if compiled!=actual:
            raise ValueError(f"runtime {name} отличается от опубликованного исполняемого кода")
        metadata[name]=dict(address=address,artifact=url,runtime=code(e,address))
    return metadata


def swap(e,pool,i,j,tokens,dx,expected):
    source,target=tokens[i],tokens[j]
    sender=HOLDERS[source]
    bal=calldata("balanceOf(address)",["address"],[sender])
    approve=calldata("approve(address,uint256)",["address","uint256"],[pool,dx])
    def exchange(limit):
        return calldata("swap(uint8,uint8,uint256,uint256,uint256)",["uint8","uint8","uint256","uint256","uint256"],
                        [i,j,dx,limit,2**256-1])
    quote=calldata("calculateSwap(uint8,uint8,uint256)",["uint8","uint8","uint256"],[i,j,dx])
    pairs=[(source,approve),(pool,quote),(source,bal),(target,bal),(pool,exchange(expected+1)),
           (pool,exchange(expected)),(source,bal),(target,bal)]
    calls=[{"from":sender,"to":to,"data":data} for to,data in pairs]
    block,=rpc(e,"eth_simulateV1",[{"blockStateCalls":[{"calls":calls}],"validation":False},e["selector"]])
    results=block["calls"]
    if block["parentHash"]!=e["selector"]["blockHash"] or [r["status"] for r in results]!=["0x1"]*4+["0x0"]+["0x1"]*3:
        raise ValueError(f"ошибка исполнения или защиты {pool}, {source}, {dx}: {[(r['status'],r.get('error')) for r in results]}")
    n=lambda k:int(results[k]["returnData"],16)
    if n(1)!=n(5) or n(5)!=expected or n(7)-n(3)!=expected or n(2)-n(6)!=dx:
        raise ValueError("котировка, возврат swap и балансы расходятся")
    return dict(i=i,j=j,dx=str(dx),out=str(expected),holder=sender,calls=calls,results=results)


def main():
    e=read(FOLDER/"ethereum.json")
    c=Capture(e)
    response=requests.get(DOC,timeout=15)
    response.raise_for_status()
    mainnet=response.text.split("## Mainnet",1)[1].split("\n## ")[0]
    docs=dict(re.findall(r"\| `([^`]+)`\s*\| \[(0x[\da-fA-F]{40})\]", mainnet))
    registry=docs["PoolRegistry"].lower()
    pools={addr.lower():{"document_title":name} for name,addr in docs.items()
           if re.fullmatch(r"[a-zA-Z0-9]*Pool(?:V\d+|Updated)?",name)}
    length,=c.one(registry,"getPoolsLength()",returns=["uint256"])
    records=take(c,[(registry,"getPoolDataAtIndex(uint256)",["uint256"],[i]) for i in range(length)])
    for index,raw in enumerate(records):
        if not raw:
            raise ValueError("запись реестра отсутствует")
        r,=decode([TUPLE],raw)
        pool=pools.setdefault(r[0],{})
        pool["registry"]=dict(index=index,name=r[3].rstrip(b"\0").decode(),lp=r[1],asset_type=r[2],target=r[4],
                              tokens=list(r[5]),underlying=list(r[6]),base_pool=r[7],deposit=r[8],
                              approved=r[9],removed=r[10],guarded=r[11])
    verified=implementation(e)
    active=[]
    for pool,entry in pools.items():
        paused,storage,token=take(c,[(pool,"paused()",[],[]),(pool,"swapStorage()",[],[]),
                                     (pool,"getToken(uint8)",["uint8"],[0])])
        entry["paused"]=decode(["bool"],paused)[0] if paused else None
        entry["storage_words"]=len(storage)//32 if storage else None
        entry["token0"]=decode(["address"],token)[0] if token else None
        if not token:
            continue
        approve_reason="Pausable: paused"
        evidence=[]
        for i,j in [(0,1),(1,0)]:
            raw,=c.batch([(pool,"swap(uint8,uint8,uint256,uint256,uint256)",
                          ["uint8","uint8","uint256","uint256","uint256"],[i,j,1,1,2**256-1])])
            reply=c.calls[-1]["response"]
            reason=reply.get("message","") if isinstance(reply,dict) else ""
            if isinstance(reply,dict):
                data=reply.get("data")
                if isinstance(data,str) and data.startswith("0x08c379a0"):
                    reason=decode(["string"],bytes.fromhex(data[10:]))[0]
            evidence.append(dict(i=i,j=j,ok=raw is not None,reason=reason))
        entry["swap_probe"]=evidence
        entry["verified_pause"]=entry["paused"] is True and all(
            not case["ok"] and approve_reason.lower() in case["reason"].lower() for case in evidence)
        if entry["paused"] is not False or "registry" not in entry:
            continue
        row=entry["registry"]
        tokens=row["tokens"]
        balances=[c.one(pool,"getTokenBalance(uint8)",["uint8"],[i],["uint256"])[0] for i in range(len(tokens))]
        entry["balances"]=list(map(str,balances))
        raw=rpc(e,"eth_getCode",[pool,e["selector"]])
        prefix,suffix="0x363d3d373d3d3d363d73","5af43d82803e903d91602b57fd5bf3"
        if len(raw)==92 and raw.startswith(prefix) and raw.endswith(suffix):
            entry["clone_target"]="0x"+raw[len(prefix):len(prefix)+40]
        if (row["removed"] or entry.get("clone_target")!=verified["PermissionlessSwap"]["address"]
                or any(b==0 for b in balances)):
            continue
        if any(c.one(pool,"getToken(uint8)",["uint8"],[i],["address"])[0]!=t for i,t in enumerate(tokens)):
            raise ValueError("монеты пула расходятся с реестром")
        entry["decimals"]=[c.one(t,"decimals()",returns=["uint8"])[0] for t in tokens]
        entry["a_precise"]=str(c.one(pool,"getAPrecise()",returns=["uint256"])[0])
        entry["fee"]=str(int.from_bytes(storage[4*32:5*32]))
        cases=[]
        for i,balance in enumerate(balances):
            for j in range(len(tokens)):
                if j==i:
                    continue
                for divisor in (1000,100):
                    dx=balance//divisor
                    out,=c.one(pool,"calculateSwap(uint8,uint8,uint256)",["uint8","uint8","uint256"],[i,j,dx],["uint256"])
                    if not out:
                        raise ValueError("пустая котировка")
                    cases.append(swap(e,pool,i,j,tokens,dx,out))
        entry["swaps"]=cases
        code(e,pool)
        active.append(pool)
    write(FOLDER/"saddle_ethereum.json",dict(source=DOC,registry=registry,chain_id=1,selector=e["selector"],
          endpoint=e["endpoint"],registry_count=length,pools=pools,calls=c.calls,verified=verified,active=active))
    write(FOLDER/"ethereum.json",e)
    path=ROOT/"config/first_run_dex.json"
    rows=[r for r in read(path) if (r["id"],r["network"])!=("saddle-finance","ethereum")]
    if active:
        rows.append(dict(id="saddle-finance",network="ethereum",chain_id=1,source=DOC,
                         settings={"dex.kind":"saddle_stableswap","dex.stable_pools":active}))
    write(path,rows)
    print("registry",length,"unique",len(pools),"paused",sum(p["paused"] is True for p in pools.values()),
          "both_swap_reverts",sum(p.get("verified_pause",False) for p in pools.values()))
    print([(p,entry["paused"]) for p,entry in pools.items() if entry["paused"] is not True])


if __name__=="__main__":
    main()
