"""Сверяет канонические рынки Fynd AMM, авторизацию forward и происхождение ставок VectaSwap."""
import gzip
import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from time import sleep
import requests
from eth_abi import decode, encode
from eth_utils import keccak
from record_umia_base import DATA

CORE="0x00000000000014aa86c5d3c41765bb24e11bd701"
OLD="0x55b703eed01b35641963da2fb2e14885993605a3"
NEW="0x550cfe1292d0d9c0d28f232b3f42edb4439b1d09"
VECTA="0xe84fa17c2ea423e0dea3c986ee3e1560d684bb2d"
NODES={"base":"https://base-rpc.publicnode.com","ethereum":"https://mainnet.gateway.tenderly.co"}
SIG="PoolInitialized(bytes32,(address,address,bytes32),int32,uint96)"
CACHE=Path("../outputs/base-ethereum-v2-20261126/base-post-spectra/history")
def rpc(ep,method,params):
    for turn in range(5):
        r=requests.post(ep,json=dict(id=1,jsonrpc="2.0",method=method,params=params),timeout=25)
        if r.ok and "result" in r.json():return r.json()["result"]
        if turn==4:raise ValueError(f"{ep} {method}: {r.text[:160]}")
        sleep(1+turn)


def events(chain,addr,signature,first,last):
    ep=NODES[chain];topic="0x"+keccak(text=signature).hex()
    page=45000 if chain=="base" else 900000
    cache=CACHE/chain/addr/topic;cache.mkdir(exist_ok=True,parents=True)
    def load(a):
        end=min(last,a+page-1);f=cache/f"{a}-{end}.json"
        if not f.exists():
            rows=rpc(ep,"eth_getLogs",[dict(address=addr,topics=[topic],fromBlock=hex(a),toBlock=hex(end))])
            f.write_text(json.dumps(rows),encoding="utf8")
        rows=json.loads(f.read_bytes())
        if any(r["address"].lower()!=addr or r["topics"][0]!=topic or r.get("removed") or
               not a<=int(r["blockNumber"],16)<=end for r in rows):raise ValueError("неверная страница")
        return rows
    with ThreadPoolExecutor(max_workers=3) as ex:rows=[r for group in ex.map(load,range(first,last+1,page)) for r in group]
    if len({(r["blockHash"],r["logIndex"]) for r in rows})!=len(rows):raise ValueError("дублированное событие")
    return rows


def fynd(chain):
    ep=NODES[chain];snap=json.loads((DATA/(chain+".json")).read_bytes());at=snap["selector"];last=snap["block"]
    if rpc(ep,"eth_getBlockByNumber",[hex(last),False])["hash"]!=at["blockHash"]:raise ValueError("не совпадает канон")
    deployments={};roots=([NEW] if chain=="base" else [OLD,NEW])
    for a in roots:
        url=f"https://sourcify.dev/server/v2/contract/{snap['chain_id']}/{a}"
        r=requests.get(url,params={"fields":"runtimeBytecode.onchainBytecode,compilation,deployment,sources,abi"},timeout=25);r.raise_for_status();v=r.json()
        actual=rpc(ep,"eth_getCode",[a,at]);b=bytes.fromhex(actual[2:])
        if v.get("runtimeMatch") not in ("match","exact_match") or v["runtimeBytecode"]["onchainBytecode"].lower()!=actual.lower():raise ValueError("другой runtime extension")
        name=v["compilation"]["fullyQualifiedName"].split(":")[0];source=v["sources"][name]["content"]
        if "SwapMustHappenThroughForward" not in source:raise ValueError("изменились условия исполнения")
        deployments[a]=dict(source=url,bytes=len(b),runtime="0x"+keccak(b).hex(),deployment=v["deployment"],compiled=name)
    first=min(int(d["deployment"]["blockNumber"]) for d in deployments.values())
    records=events(chain,CORE,SIG,first,last);markets=[];seen=set()
    for r in records:
        pool,key,tick,ratio=decode(["bytes32","(address,address,bytes32)","int32","uint96"],bytes.fromhex(r["data"][2:]))
        ext="0x"+key[2][:20].hex()
        if ext not in deployments:continue
        if keccak(encode(["(address,address,bytes32)"],[key]))!=pool or pool in seen:raise ValueError("подмена PoolId")
        seen.add(pool)
        if rpc(ep,"eth_getBlockByNumber",[r["blockNumber"],False])["hash"]!=r["blockHash"]:raise ValueError("регистрация вне канона")
        word=rpc(ep,"eth_getStorageAt",[ext,"0x"+pool.hex(),at])
        params=["bytes32","(address,address,bytes32)","bytes32"]
        raw="0x"+(keccak(text="beforeSwap("+",".join(params)+")")[:4]+encode(params,[bytes(32),key,(1).to_bytes(32,"big")])).hex()
        unsigned={"to":ext,"data":raw}
        reply=requests.post(ep,json=dict(id=1,jsonrpc="2.0",method="eth_call",params=[unsigned,at]),timeout=20);reply.raise_for_status()
        err=reply.json().get("error",{});expected="0x"+keccak(text="SwapMustHappenThroughForward()")[:4].hex()
        if err.get("data")!=expected:raise ValueError(("неверный контроль forward",err))
        markets.append(dict(id="0x"+pool.hex(),controller=word[:42],updated=(int(word,16)>>64)&0xffffffff,
                            owner_fee_x64=str(int(word,16)&(2**64-1)) if ext==NEW else None,
                            storage=word,unsigned_call=unsigned,unsigned_error=err,tokens=list(key[:2]),config="0x"+key[2].hex(),extension=ext,initial_tick=tick,initial_sqrt_ratio=str(ratio),log=r))
    health="https://fynd-api.propellerheads.xyz/v1/"+chain+"/health"
    h=requests.get(health,timeout=15)
    return dict(chain=chain,selector=at,height=last,start=first,core=CORE,initialize_logs=len(records),deployments=deployments,markets=markets,
                unauthenticated_health=dict(url=health,status=h.status_code),guide="https://docs.fynd.xyz/get-started/fynd-amm.md",access="https://docs.fynd.xyz/get-started/hosted-api.md")


def fee_tiers():
    from record_spectra_base import AT,BLOCK,call,many,read
    if read("eth_getBlockByNumber",[hex(BLOCK),False])["hash"]!=AT["blockHash"]:raise ValueError("другая высота")
    logs=events("base",VECTA,"FeeAmountEnabled(uint24,int24)",53245146,BLOCK)
    tiers={}
    for r in logs:
        fee,step=[int(x,16) for x in r["topics"][1:]]
        if fee in tiers or many([call(VECTA,"feeAmountTickSpacing(uint24)","int24",fee)])!=[step]:raise ValueError("ABI/повтор fee tier")
        if read("eth_getBlockByNumber",[r["blockNumber"],False])["hash"]!=r["blockHash"]:raise ValueError("ставка вне канона")
        tiers[fee]=step
    return dict(selector=AT,factory=VECTA,tiers=tiers,logs=logs)


if __name__=="__main__":
    import argparse
    ps=argparse.ArgumentParser(description=__doc__);ps.add_argument("target",choices=["base","ethereum","vecta"]);args=ps.parse_args()
    v=fee_tiers() if args.target=="vecta" else fynd(args.target)
    f="vecta_base_tiers" if args.target=="vecta" else "fynd_"+args.target+"_markets"
    (DATA/(f+".json.gz")).write_bytes(gzip.compress(json.dumps(v,separators=(",",":")).encode(),mtime=0))
