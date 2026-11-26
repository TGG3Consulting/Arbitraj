"""Record SquadSwap V3 on Base using the official deployment table and the repository V5 code."""
import argparse
import json
from pathlib import Path

from bs4 import BeautifulSoup
from eth_abi import decode
from eth_utils import keccak
import requests

from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT, calldata
from record_metric_chains import VIRTUAL
from record_pool_executor_controls import record
from record_rubicon_clmm import FOLDER, pool_address

DOC = "https://docs.squadswap.com/platform/developers/smart-contracts/base-contracts"
WETH = "0x4200000000000000000000000000000000000006"
STABLES = ["0xfde4c96c8593536e31f229ea8f37b2ada2699bb2","0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"]


def rpc(c, method, params):
    response = c.session.post(c.endpoint,json=dict(jsonrpc="2.0",id=1,method=method,params=params),timeout=24)
    response.raise_for_status()
    result=response.json()
    if "error" in result:
        raise ValueError(result["error"])
    return result["result"]


def official():
    response = requests.get(DOC, timeout=25)
    response.raise_for_status()
    rows = BeautifulSoup(response.content,"html.parser").select("[role=row],tr")
    fields = {}
    for row in rows:
        parts = row.get_text("|",strip=True).split("|")
        if len(parts)==2:
            fields[parts[0]] = parts[1].lower()
    return {k:fields[k] for k in ["SquadV3Factory","SquadV3PoolDeployer","TickLens","QuoterV2","v3InitCodeHash"]}


def collect():
    snapshot=json.loads((FOLDER/"base.json").read_bytes())
    c=Capture(snapshot)
    d=official()
    factory=d["SquadV3Factory"]
    deployer,=c.one(factory,"poolDeployer()",returns=["address"])
    assert deployer == d["SquadV3PoolDeployer"]
    virtual=(ROOT/"contracts/out/VirtualQuoter.bin-runtime").read_text().strip().removeprefix("0x")
    vhash="0x"+keccak(bytes.fromhex(virtual)).hex()
    orig="https://sourcify.dev/server/v2/contract/8453/"
    def verified(pool):
        response=requests.get(orig+pool, params={"fields":"compilation,sources,runtimeBytecode.onchainBytecode,creationBytecode.onchainBytecode"},timeout=24)
        response.raise_for_status()
        info=response.json()
        source=info["sources"][info["compilation"]["fullyQualifiedName"].split(":")[0]]["content"]
        runtime=rpc(c,"eth_getCode",[pool,c.selector])
        assert info["runtimeMatch"] and info["runtimeBytecode"]["onchainBytecode"].lower()==runtime.lower()
        assert "uint24 public immutable override fee;" in source and "squadV3SwapCallback" in source
        h=keccak(bytes.fromhex(info["creationBytecode"]["onchainBytecode"][2:]))
        return dict(source=orig+pool,runtime_keccak="0x"+keccak(bytes.fromhex(runtime[2:])).hex(),init_hash="0x"+h.hex())
    tiers={str(f):c.one(factory,"feeAmountTickSpacing(uint24)",["uint24"],[f],["int24"])[0] for f in (100,500,2500,10000)}
    targets=[]
    markets=[]
    for coin in STABLES:
        for f in map(int,tiers):
            pool,=c.one(factory,"getPool(address,address,uint24)",["address","address","uint24"],[WETH,coin,f],["address"])
            if int(pool,16)==0:continue
            tokens=[c.one(pool,key+"()",returns=["address"])[0] for key in ("token0","token1")]
            assert set(tokens)=={WETH,coin}
            assert c.one(pool,"fee()",returns=["uint24"])[0]==f
            attestation=verified(pool)
            assert pool_address(deployer,tokens,f,bytes.fromhex(attestation["init_hash"][2:]))==pool
            slot=c.one(pool,"slot0()",returns=["uint160","int24","uint16","uint16","uint16","uint32","bool"])
            spacing=c.one(pool,"tickSpacing()",returns=["int24"])[0]
            assert spacing==tiers[str(f)]
            word=slot[1]//spacing//256
            ticks,=c.one(d["TickLens"],"getPopulatedTicksInWord(address,int16)",["address","int16"],[pool,word],["(int24,int128,uint128)[]"])
            requested=[]
            bounds=[]
            for side in (0,1):
                sizes=(10**10,10**11,10**12,10**14,10**16) if tokens[side]==WETH else (100,1000,100000,1000000,10000000)
                for amt in sizes:
                    call={"to":VIRTUAL,"data":calldata("quoteExactIn(address,bool,uint256)",["address","bool","uint256"],[pool,side==0,amt])}
                    try:
                        raw=rpc(c,"eth_call",[call,c.selector,{VIRTUAL:{"code":"0x"+virtual}}])
                    except ValueError as error:
                        detail=error.args[0]
                        if not isinstance(detail,dict) or detail.get("code")!=3:
                            raise
                        bounds.append(dict(side=side,input=str(amt),reverted=detail))
                        continue
                    used,out=decode(["uint256","uint256"],bytes.fromhex(raw[2:]))
                    bounds.append(dict(side=side,input=str(amt),used=str(used),out=str(out)))
                full=[b for b in bounds if b["side"]==side and b.get("used")==b["input"] and int(b["out"])]
                if full:
                    for b in [full[0],full[-1]] if len(full)>1 else full:
                        requested.append(dict(side=side,input=b["input"]))
            liquidity=c.one(pool,"liquidity()",returns=["uint128"])[0]
            markets.append(dict(pool=pool,tokens=tokens,fee=f,spacing=spacing,code=attestation,liquidity=str(liquidity),
                                doc_init_address=pool_address(deployer,tokens,f,bytes.fromhex(d["v3InitCodeHash"][2:])),
                                lens_word=word,ticks=ticks,quotes=bounds))
            if requested:
                targets.append(dict(idx=len(markets)-1,pool=pool,tokens=tokens,funding_weth=str(10**17),quotes=requested))
    replay=record(dict(kind="v3",network="base",hash=c.selector["blockHash"],block=snapshot["block"],
                  contracts={"factory":factory},quoter_runtime_keccak=vhash,pools=targets))
    for m in replay["pools"]:
        if not any(row.get("sold") for row in m["cases"]):
            raise ValueError((m["pool"],m["cases"]))
    return dict(source=DOC,official=d,tiers=tiers,selector=c.selector,block=snapshot["block"],markets=markets,execution=replay,reads=c.calls)


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("output",type=Path)
    p.parse_args().output.write_text(json.dumps(collect(),indent=2)+"\n",encoding="utf8",newline="\n")
