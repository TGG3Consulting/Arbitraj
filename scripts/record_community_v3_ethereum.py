"""Audit CommunityV3's Ethereum factory, fee tiers, every deployed pool and V5 wallet settlement."""
import argparse
import hashlib
import json
import re
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak
import requests

from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import ROOT, calldata
from record_pool_executor_controls import record
from record_rubicon_clmm import FOLDER, INDEX, FEE, CREATED, pool_address
from record_woo_base import rpc

FACTORY="0x00a38533c68327abe0cc265436e036066e2d6b38"
SAMPLE="0xbfd380e841f2fbd0e5a22fedc7c24a916bc981dc"
WETH="0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
USDC="0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"


def source(addr, snap):
    url="https://eth.blockscout.com/api/v2/smart-contracts/"+addr
    response=requests.get(url,timeout=22)
    response.raise_for_status()
    j=response.json()
    code=rpc(snap,"eth_getCode",[addr,snap["selector"]])
    assert j["is_verified"] and not j["is_changed_bytecode"] and j["deployed_bytecode"].lower()==code.lower()
    files={Path(x["file_path"]).name:x["source_code"] for x in j["additional_sources"]}
    assert len(files)==len(j["additional_sources"])
    files[j["name"]+".sol"]=j["source_code"]
    return files, dict(source=url,compiler=j["compiler_version"],file_count=len(files),
                       runtime="0x"+keccak(bytes.fromhex(code[2:])).hex(),creation="0x"+keccak(bytes.fromhex(j["creation_bytecode"][2:])).hex())


def origin(c, snap):
    proof={}
    for label,a,ref in [('factory',FACTORY,'0x1f98431c8ad98523631ae4a59f267346ea31f984'),('pool',SAMPLE,'0x88e6a0c2ddd26feeb64f039a2c41296fcb3f5640')]:
        actual,details=source(a,snap)
        reference,original=source(ref,snap)
        assert details['creation']==original['creation']
        assert actual.keys()==reference.keys() and details['compiler'].removeprefix("v")==original['compiler'].removeprefix("v")
        for name in actual:
            if label=='factory' and name in ('FullMath.sol','Oracle.sol','TickMath.sol','Position.sol','Tick.sol'):
                # Both verified compilers are 0.7.6. Only the upper pragma bound and one documentation line differ.
                reference[name]=reference[name].replace(' <0.8.0;', ';')
                if name=='Tick.sol':
                    reference[name]=re.sub(r'^\s*/// @param tickCumulative The tick \* time elapsed since the pool was first[^\n]*\n', '\n',reference[name], flags=re.M)
                    reference[name]=reference[name].replace('\n\n    /// @param','\n    /// @param')
            if actual[name]!=reference[name]:
                raise ValueError(f"Source mismatch {label}/{name}")
        proof[label]=dict(deployed=details,reference=original,source_digest=hashlib.sha256(json.dumps(actual,sort_keys=True).encode()).hexdigest())
    return proof


def collect():
    snap=json.loads((FOLDER/"ethereum.json").read_bytes())
    c=Capture(snap)
    proof=origin(c,snap)
    init=bytes.fromhex(proof['pool']['deployed']['creation'][2:])
    logs={}
    for topic in (FEE,CREATED):
        entries=[]
        for page in range(1,200):
            response=requests.get(INDEX,params=dict(module="logs",action="getLogs",fromBlock=0,toBlock=snap['block'],address=FACTORY,topic0=topic,page=page,offset=100),timeout=22)
            response.raise_for_status(); j=response.json()
            if not isinstance(j['result'],list):raise ValueError(str(j)[:160])
            entries.extend(j['result'])
            if len(j['result'])<100:break
        logs[topic]=entries
    pools=[]
    fees={}
    for ev in logs[FEE]:
        h=ev['blockNumber']
        canonical=rpc(snap,"eth_getBlockByNumber",[h,False])['hash']
        real=rpc(snap,"eth_getLogs",[{'address':FACTORY,'blockHash':canonical,'topics':[FEE]}])
        assert any(r['topics']==ev['topics'] and r['transactionHash']==ev['transactionHash'] for r in real)
        fee=int(ev['topics'][1],16)
        spacing=int(ev['topics'][2],16)
        assert c.one(FACTORY,'feeAmountTickSpacing(uint24)',['uint24'],[fee],['int24'])[0]==spacing
        fees[str(fee)]=dict(spacing=spacing,tx=ev['transactionHash'],block=canonical)
    for ev in logs[CREATED]:
        a,b=['0x'+t[-40:] for t in ev['topics'][1:3]]
        f=int(ev['topics'][3],16)
        (spacing,pool)=decode(['int24','address'],bytes.fromhex(ev['data'][2:]))
        assert pool_address(FACTORY,[a,b],f,init)==pool and fees[str(f)]['spacing']==spacing
        pools.append(dict(pool=pool,tokens=[a,b],fee=f))
    count=rpc(snap,"eth_getTransactionCount",[FACTORY,snap["selector"]])
    assert len({p['pool'] for p in pools})==len(pools)==int(count,16)-1
    lookups=posts(c,[(FACTORY,'getPool(address,address,uint24)',['address','address','uint24'],[*p['tokens'],p['fee']]) for p in pools])
    depths=posts(c,[(p['pool'],'liquidity()',[],[]) for p in pools])
    for p,reg,lq in zip(pools,lookups,depths,strict=True):
        assert decode(['address'],reg)[0]==p['pool']
        p['liquidity']=str(decode(['uint128'],lq)[0])
    profiles=json.loads((ROOT/'config/first_run_dex.json').read_bytes())
    lens=next(p['settings']['dex.tick_lens'] for p in profiles if p['id']=='uniswap_v3')
    virt=(ROOT/'contracts/out/VirtualQuoter.bin-runtime').read_text().strip().removeprefix('0x')
    top=[SAMPLE,'0x3338087046b65ab1bab72aa217d27c059b9a885a','0x49b7b66457297f9e0e763bbb151b6db0c149e988']
    targets=[]
    for p in [next(p for p in pools if p['pool']==a) for a in top]:
        side=p['tokens'].index(USDC)
        slot=c.one(p['pool'],'slot0()',returns=['uint160','int24','uint16','uint16','uint16','uint8','bool'])
        word=slot[1]//fees[str(p['fee'])]['spacing']//256
        ticks,=c.one(lens,'getPopulatedTicksInWord(address,int16)',['address','int16'],[p['pool'],word],['(int24,int128,uint128)[]'])
        p['lens_ticks']=ticks
        targets.append(dict(idx=len(targets),pool=p['pool'],tokens=p['tokens'],funding_weth=str(10**17),quotes=[dict(side=side,input=str(a)) for a in (10**4,10**6,10**7)]))
    execution=record(dict(kind='v3',network='ethereum',hash=snap['selector']['blockHash'],block=snap['block'],contracts={'factory':FACTORY},quoter_runtime_keccak='0x'+keccak(bytes.fromhex(virt)).hex(),pools=targets))
    if not any('sold' in b for m in execution['pools'] for b in m['cases']):raise ValueError(execution['pools'])
    return dict(chain_id=1,block=snap['block'],selector=snap['selector'],registry=FACTORY,nonce=count,
                tiers=fees,pools=pools,proof=proof,tick_lens=lens,execution=execution)


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output',type=Path)
    parser.parse_args().output.write_text(json.dumps(collect(),indent=2)+'\n',encoding='utf8',newline='\n')
