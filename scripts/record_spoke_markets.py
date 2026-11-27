"""Канонический реестр Maverick V1 и Topaz CL по официальным адресам Base/Ethereum."""
import argparse
import gzip
import json
from pathlib import Path
import requests
from bs4 import BeautifulSoup
from eth_abi import decode,encode
from eth_utils import keccak
from record_fynd_markets import DATA,NODES,rpc
from record_spectra_base import call
from record_omni_flux_registry import data

MAV="https://docs.mav.xyz/technical-reference/contract-addresses/v1-contract-addresses.md"
TOPAZ="https://www.topazdex.com/docs/contracts"
MULTI="0xca11bde05977b3631167028862be2a173976ca11"


def roles(chain,dex):
    url=MAV if dex=="maverick" else TOPAZ
    r=requests.get(url,timeout=20);r.raise_for_status()
    if dex=="maverick":
        section=r.text.split("## "+("Base" if chain=="base" else "Ethereum Mainnet"),1)[1].split("##",1)[0]
        table=BeautifulSoup(section,"html.parser").find("table")
        labels=["Factory","Router","PoolInformation","Position"]
    else:
        html=BeautifulSoup(r.content,"html.parser")
        table=next(t for t in html.select("table") if t.find_previous(["h2","h3","h4"]).get_text(strip=True).lower()==chain+": deployed contracts")
        labels=["CLFactory","CLPool","DynamicSwapFeeModule","QuoterV2","SwapRouter"]
    tab={cols[0].get_text(strip=True):cols[-1].get_text(strip=True).lower() for tr in table.select("tr") if (cols:=tr.select("td"))}
    return {k:tab[k] for k in labels}


class Reads:
    def __init__(self,chain):
        self.node=NODES[chain]; d=json.loads((DATA/(chain+".json")).read_bytes())
        self.at=d["selector"];self.height=d["block"];self.chain=d["chain_id"]
        h=rpc(self.node,"eth_getBlockByNumber",[hex(self.height),False]);self.root=h["stateRoot"]
        if h["hash"]!=self.at["blockHash"] or int(rpc(self.node,"eth_chainId",[]),16)!=self.chain:raise ValueError("нет подтверждённого блока сети")
    def many(self,calls,optional=False):
        result=[]
        for i in range(0,len(calls),100):
            group=calls[i:i+100]; payload=data("aggregate3((address,bool,bytes)[])",["(address,bool,bytes)[]"],[[(a,True,b) for a,b,_ in group]])
            raw=rpc(self.node,"eth_call",[dict(to=MULTI,data="0x"+payload.hex()),self.at])
            group_results=decode(["(bool,bytes)[]"],bytes.fromhex(raw[2:]))[0]
            for (ok,raw),(a,b,t) in zip(group_results,group,strict=True):
                if not ok or not raw:
                    if not optional:raise ValueError((a,b.hex(),"ABI не вернулся"))
                    result.append(None)
                else:result.append(decode([t],raw)[0])
        return result
    def code(self,a):
        raw=bytes.fromhex(rpc(self.node,"eth_getCode",[a,self.at])[2:])
        if not raw:raise ValueError((a,"нет кода"))
        return dict(bytes=len(raw),keccak256="0x"+keccak(raw).hex())


def created(deployer,n):
    raw=n.to_bytes((n.bit_length()+7)//8,"big")
    if n>=128:raw=bytes([128+len(raw)])+raw
    tail=bytes.fromhex("94"+deployer[2:])+raw
    return "0x"+keccak(bytes([192+len(tail)])+tail)[-20:].hex()


def record(chain,dex):
    read=Reads(chain); contracts=roles(chain,dex); mav=dex=="maverick"
    f=contracts["Factory" if mav else "CLFactory"]
    sites={k:dict(address=a,**read.code(a)) for k,a in contracts.items()}
    if mav:
        nonce=int(rpc(read.node,"eth_getTransactionCount",[f,read.at]),16);addresses=[created(f,i) for i in range(1,nonce)]
        registered=read.many([call(f,"isFactoryPool(address)","bool",a) for a in addresses])
        if not all(registered):raise ValueError("CREATE с nonce фабрики без регистрации")
        abi=[("factory()","address"),("tokenA()","address"),("tokenB()","address"),("fee()","uint256"),("tickSpacing()","uint256"),
             ("binBalanceA()","uint128"),("binBalanceB()","uint128"),("getState()","(int32,uint8,uint128,uint64)")]
        catalog=dict(nonce=nonce)
    else:
        n,imp,mod,spaces=read.many([call(f,s,t) for s,t in (("allPoolsLength()","uint256"),("poolImplementation()","address"),("swapFeeModule()","address"),("tickSpacings()","int24[]"))])
        if (imp,mod)!=(contracts["CLPool"],contracts["DynamicSwapFeeModule"]):raise ValueError("заменены реализация или модуль")
        addresses=read.many([call(f,"allPools(uint256)","address",i) for i in range(n)])
        abi=[("factory()","address"),("token0()","address"),("token1()","address"),("fee()","uint24"),("tickSpacing()","int24"),("liquidity()","uint128")]
        catalog=dict(spaces=spaces)
    if len(addresses)!=len(set(addresses)):raise ValueError("дублированный рынок")
    fields=iter(read.many([call(a,s,t) for a in addresses for s,t in abi]));markets=[]
    for i,a in enumerate(addresses):
        fa,t0,t1,fee,space,*rest=[next(fields) for _ in abi]
        if fa!=f or t0>=t1:raise ValueError("неверные фабрика или токены")
        m=dict(index=i,pool=a,tokens=[t0,t1],fee=str(fee),spacing=space)
        if mav:m.update(balances=list(map(str,rest[:2])),active_tick=rest[2][0],status=rest[2][1],bin_count=rest[2][2],protocol_fee=rest[2][3])
        else:m["liquidity"]=str(rest[0])
        markets.append(m)
    if not mav:
        checks=iter(read.many([v for m in markets for v in (call(f,"getPool(address,address,int24)","address",*m["tokens"],m["spacing"]),call(f,"getSwapFee(address)","uint24",m["pool"]),call(f,"getUnstakedFee(address)","uint24",m["pool"]))]))
        for m in markets:
            if next(checks)!=m["pool"] or next(checks)!=int(m["fee"]):raise ValueError("плата или фабрика")
            m["unstaked"]=next(checks)
    return dict(chain=chain,chain_id=read.chain,height=read.height,selector=read.at,state_root=read.root,source=MAV if mav else TOPAZ,roles=sites,catalog=catalog,markets=markets)


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument("dex",choices=["topaz","maverick"]);ap.add_argument("chain",choices=["base","ethereum"]);a=ap.parse_args()
    d=record(a.chain,a.dex);(DATA/f"{a.dex}_v1_{a.chain}_directory.json.gz").write_bytes(gzip.compress(json.dumps(d,separators=(",",":")).encode(),mtime=0))
