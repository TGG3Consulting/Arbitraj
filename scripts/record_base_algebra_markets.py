"""Canonical Algebra V1.9 and Integral Base samples: pool identity, fee sources and actual two-way V5 fills."""
import argparse
import json
from pathlib import Path

from eth_utils import keccak
from record_aero_basic_base import round_trip
from record_balancer_markets import Capture
from record_base_classic_directory import ENDPOINTS
from record_base_slipstream_executor import RUNTIME, ROOT, rpc
from record_hydrex_integral import VIRTUAL, VIRTUAL_FILE
from record_solidly_v3 import HOLDERS
from record_synthswap_oracle import capture as synth_oracle

MARKETS={
    "synthswap-v3":("0xa37359e63d1aa44c0acb2a4605d3b45785c97ee3","0xe0712c087ecb8a0dd20914626152ebf4890708c2"),
    "henjin":("0x4963818c35d5793d771bf8091c750b5a71ed101b","0xe301ca0e15446d7f2eea46e8d158c5bda1f760f2"),
    "kim-v4-base":("0x2f0d41f94d5d1550b79a83d2fe85c82d68c5a3ca","0xbbd7d3f242fe8f4eb318c96b2fac0c459ecca908"),
    "treble-v4":("0x6e606cf94a4ddc01aeed2fce16d1b4f5b33e0a31","0x09f3805c6743d9d0548ffe5657a985a057f82a7b"),
}
ZERO="0x"+"00"*20
def capture(dex,endpoint):
    e=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes())
    e["endpoint"]=endpoint;c=Capture(e);tag=e["selector"]
    header=rpc(endpoint,"eth_getBlockByHash",[tag["blockHash"],False])
    if rpc(endpoint,"eth_chainId",[])!="0x2105" or int(header["number"],16)!=e["block"]:
        raise ValueError("wrong chain or header")
    fac,pool=MARKETS[dex];integral=dex!="synthswap-v3"
    addr=lambda to,method,types=(),args=():c.one(to,method,types,args,["address"])[0]
    a,b=(addr(pool,token) for token in ("token0()","token1()"))
    if a>=b or addr(pool,"factory()")!=fac or addr(fac,"poolByPair(address,address)",["address","address"],[a,b])!=pool:
        raise ValueError("wrong registered pool")
    roles={"factory":fac,"pool":pool,"deployer":addr(fac,"poolDeployer()")}
    gs=c.one(pool,"globalState()",returns=["uint160","int24","uint16","uint8","uint16","bool"] if integral else
             ["uint160","int24","uint16","uint16","uint8","uint8","bool"])
    reads={}
    if integral:
        roles.update(plugin=addr(pool,"plugin()"),plugin_factory=addr(fac,"defaultPluginFactory()"))
        plug=roles["plugin"]
        if addr(plug,"pool()")!=pool:raise ValueError("foreign plugin")
        rr=[(roles["plugin_factory"],"pluginByPool(address)",["address"],[pool])]
        rr.extend((plug,m,[],[]) for m in ["getCurrentFee()","defaultPluginConfig()","incentive()","rebalanceManager()","limitOrderManager()","getSecurityRegistry()"])
        rr.append((fac,"POOL_INIT_CODE_HASH()",[],[]))
    else:
        roles.update(operator=addr(pool,"dataStorageOperator()"),quoter="0x9c8f11c113f1fe7959c838a5f6b43e5e6223320c")
        rr=[(pool,"activeIncentive()",[],[]),(roles["operator"],"feeConfig()",[],[]),(roles["quoter"],"factory()",[],[])]
    for row,bits in zip(rr,c.batch(rr),strict=True):
        reads[row[1]]=None if bits is None else "0x"+bits.hex()
    if dex=="treble-v4":
        for role,signature in [("limit_order_manager","limitOrderManager()"),("security_registry","getSecurityRegistry()")]:
            val=reads[signature]
            if val and int(val,16):
                roles[role]="0x"+val[-40:]
        if "security_registry" in roles:
            status,=c.one(roles["security_registry"],"getPoolStatus(address)",["address"],[pool],["uint8"])
            reads["security_status"]=status
    if dex=="synthswap-v3" and int(reads["activeIncentive()"],16):
        roles["incentive"]="0x"+reads["activeIncentive()"][-40:]
    oracle=synth_oracle(c,endpoint,pool,gs,roles,header) if dex=="synthswap-v3" else None
    verified={}
    for name,address in roles.items():
        code=rpc(endpoint,"eth_getCode",[address,tag])
        if code in ("0x","0x0"):raise ValueError("no code for "+name)
        verified[name]={"address":address,"keccak":"0x"+keccak(hexstr=code).hex(),"size":(len(code)-2)//2}
    liquidity,=c.one(pool,"liquidity()",returns=["uint128"])
    spacing,=c.one(pool,"tickSpacing()",returns=["int24"])
    token=next(t for t in (a,b) if t in HOLDERS["base"]);other=b if token==a else a
    decimals,=c.one(token,"decimals()",returns=["uint8"])
    # 0.000001 and 0.00001 WETH; USDC 0.01 and 0.1. Sizes only describe this snapshot.
    sizes=[max(1,10**decimals//x) for x in ((10**6,10**5) if decimals==18 else (100,10))]
    runtime=RUNTIME.read_text().strip().removeprefix("0x");virt=VIRTUAL_FILE.read_text().strip().removeprefix("0x")
    cases=[round_trip(e,pool,token,other,size,int(header["timestamp"],16)+120,runtime,kind=2,
                      owner=HOLDERS["base"][token],algebra=(VIRTUAL,virt,a),time=(oracle["timestamp"]+oracle["block_step"] if oracle else None)) for size in sizes]
    return dict(id=dex,oracle=oracle,chain_id=8453,selector=tag,state_root=header["stateRoot"],roles=verified,reads=reads,raw=c.calls,
                tokens=[a,b],global_state=list(gs),liquidity=str(liquidity),tick_spacing=spacing,cases=cases,
                v5="0x"+keccak(hexstr=runtime).hex(),virtual="0x"+keccak(hexstr=virt).hex())

if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("id",choices=MARKETS);parser.add_argument("output",type=Path);args=parser.parse_args()
    for endpoint in ENDPOINTS:
        try:
            evidence=capture(args.id,endpoint)
            args.output.write_text(json.dumps(evidence,indent=2)+"\n",encoding="utf8",newline="\n")
            break
        except (ValueError,OSError,KeyError) as exc:
            print(endpoint,str(exc)[:240],flush=True)
    else:raise ValueError("no accepted wallet cases")
