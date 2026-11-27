"""Перечисляет реальные спотовые vault Umia из Hub и отдельно воспроизводит их выплаты через Router и V5."""
import argparse
import gzip
import json
from pathlib import Path

import requests
from eth_abi import encode
from eth_utils import keccak
from record_base_launch_hooks import DATA, KEY
from record_base_launch_wallets import ZERO, replay
from record_omni_v2_base import ENDPOINTS
from record_spectra_base import AT, BLOCK, call, many, read

HUB="0x120dbcdd58bb787309573e29159fe6d37a1983f6"
UPSTREAM="https://github.com/umiafinance/protocol/tree/3165f5a5bdaa23a83fbfd4894523e7ca84d53a14"


def directory():
    head=read("eth_getBlockByNumber",[hex(BLOCK),False])
    if head["hash"]!=AT["blockHash"]:raise ValueError("несовпадающий канон Base")
    profiles=json.loads(Path("config/first_run_dex.json").read_bytes())
    cfg=next(p["settings"] for p in profiles if p["chain_id"]==8453 and p["settings"]["dex.kind"]=="uniswap_v4")
    manager,view=cfg["dex.pool_manager"].lower(),cfg["dex.state_view"].lower()
    n,core=many([call(HUB,"ventureCount()","uint256"),call(HUB,"umiaMarketCore()","address")])
    ventures=many([call(HUB,"ventureById(uint256)","(uint256,address,string,uint256)",i+1) for i in range(n)])
    sites={HUB,core}
    rows=[]
    for index,venture,name,created in ventures:
        vault,token,money=many([call(HUB,"ventureLiquidityVault(address)","address",venture),call(venture,"token()","address"),
                                call(venture,"moneyToken()","address")])
        row=dict(group="umia",index=index,venture=venture,name=name,created=created,vault=vault,token=token,money=money)
        rows.append(row)
        if vault==ZERO:continue
        key,v,m,h,vt,mt=many([call(vault,"getPoolKey()",KEY)]+[call(vault,s,"address") for s in
             ("venture()","poolManager()","hook()","ventureToken()","moneyToken()")])
        if (venture,manager,key[-1],token,money)!=(v,m,h,vt,mt) or sorted(key[:2])!=sorted([token,money]):
            raise ValueError(f"{vault}: подменены роли")
        pool_id=keccak(encode([KEY],[key]))
        hook_manager,registrar,linked,liq,slot=many([call(h,"poolManager()","address"),call(h,"factory()","address"),
            call(h,"pools(bytes32)","(address,address,address)",pool_id),call(view,"getLiquidity(bytes32)","uint128",pool_id),
            call(view,"getSlot0(bytes32)","(uint160,int24,uint24,uint24)",pool_id)])
        launch,rv,operator=linked
        registered,=many([call(registrar,"isLBP(address)","bool",launch)])
        migration={}
        if not registered:
            hub,migrator,operators,active=many([call(h,"hub()","address"),call(HUB,"spotMigrator()","address"),
                call(h,"pools(bytes32)","(address,address,address,address)",pool_id),call(vault,"activeManager()","address")])
            if hub!=HUB or migrator!=launch or operators!=(launch,venture,vault,active) or active in (ZERO,vault):
                raise ValueError("не связаны оператор и миграция рынка")
            migration=dict(active=active,migrator=migrator);sites.add(active)
        if (manager,venture,vault)!=(hook_manager,rv,operator):
            raise ValueError(f"{vault}: хук не зарегистрировал данный vault и launcher")
        sites.update([h,registrar,vault,launch,venture])
        row.update(id="0x"+pool_id.hex(),key=list(key),terms={"quote":money,"launcher":launch,"factory":registrar,"operator":vault,**migration},
                   liquidity=str(liq),slot0=[str(slot[0]),*slot[1:]],sizes=[1000,100000,10**6,10**7])
    codes={}
    for address in sorted(sites):
        raw=bytes.fromhex(read("eth_getCode",[address,AT])[2:])
        if not raw:raise ValueError(f"роль без кода {address}")
        codes[address]=dict(bytes=len(raw),keccak256="0x"+keccak(raw).hex())
    return dict(chain_id=8453,height=BLOCK,selector=AT,state_root=head["stateRoot"],manager=manager,state_view=view,
                hub=HUB,decision_core=core,source=UPSTREAM,contracts=codes,rows=rows)


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action",choices=("directory","wallets"))
    parser.add_argument("output",type=Path)
    args=parser.parse_args()
    if args.action=="directory":
        result=directory()
    else:
        with gzip.open(DATA/"umia_base_directory.json.gz") as f:snapshot=json.load(f)
        with gzip.open(DATA/"umia_base_history.json.gz") as f:history=json.load(f)
        if history["selector"]!=snapshot["selector"]:raise ValueError("история из другого блока")
        for old in history["retired"]:
            venture,=many([call(old["vault"],"venture()","address")])
            r=next(r for r in snapshot["rows"] if r["venture"]==venture)
            snapshot["rows"].append(dict(old,group="umia",terms={"quote":r["money"]},sizes=r["sizes"]))
        for endpoint in ENDPOINTS:
            try:result=replay("umia",endpoint,snapshot)
            except (ValueError,requests.RequestException):
                if endpoint==ENDPOINTS[-1]:raise
            else:break
    raw=json.dumps(result,separators=(",",":")).encode()
    args.output.write_bytes(gzip.compress(raw,mtime=0) if args.output.suffix==".gz" else raw)
