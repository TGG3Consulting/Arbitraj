"""Канонические роли Zora и пулы реальных монет из опубликованного SDK и Swapped."""
import argparse
import json
from collections import Counter
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak
from record_basestonk_base import KEY, MANAGER, multi
from record_eulerswap_directory import DATA, Reader

FACTORY="0x777777751622c0d3258f214f9df38e35bf45baf3"
REGISTRY="0x777777c4c14b133858c3982d41dbf02509fc18d7"
ROUTER="0x77777777eb762cf86f634763e79d17de44330887"
VIEW="0xa3c0c9b65bad0b08107aa264b0f3db444b867a71"
UNI3="0x33128a8fc17869897dce68ed026d694621f6fdfd"
SWAP="0x"+keccak(text="Swapped(address,address,bool,(address,address,uint24,int24,address),bytes32,(bool,int256,uint160),int128,int128,bool,bytes,uint160)").hex()

def capture():
    r=Reader("base")
    sdk=json.loads((DATA/"zora_base_sdk.json").read_bytes())
    hooks,content,creator,factory_version=r.many([(REGISTRY,"getHookAddresses()",(),"address[]"),
            (FACTORY,"contentCoinHook()",(),"address"),(FACTORY,"creatorCoinHook()",(),"address"),(FACTORY,"contractVersion()",(),"string")])
    roles=[{"address":h,"hash":r.code_hash(h),"tag":tag,"version":v} for h,(tag,v) in zip(hooks,
        zip(r.many([(REGISTRY,"getHookTag(address)",(h,),"string") for h in hooks]),
            r.many([(h,"contractVersion()",(),"string") for h in hooks])))]
    # Каждая зарегистрированная касса имеет свою эволюцию ABI и свою активность.
    recent=[];coverage={}
    for h in hooks:
        try:
            logs=r.rpc("eth_getLogs",[dict(address=h,fromBlock=hex(r.height-2047),toBlock=hex(r.height))])
            count=Counter(l["topics"][0] for l in logs if l["topics"])
            for l in logs:
                if l["topics"][:1]!=[SWAP]:continue
                trusted,key,params,a0,a1,buy,ref,_=decode(["bool",KEY,"(bool,int256,uint160)","int128","int128","bool","bytes","uint160"],bytes.fromhex(l["data"][2:]))
                assert key[4]==h and "0x"+keccak(encode([KEY],[key])).hex()==l["topics"][3]
                recent.append(dict(hash=l["transactionHash"],block=l["blockHash"],index=l["logIndex"],key=key,id=l["topics"][3],sender=l["topics"][1],actual_sender=l["topics"][2],
                                 trusted=trusted,delta=[str(a0),str(a1)],buy=buy,hookdata_size=len(ref),is_exact_input=params[1]<0))
            coverage[h]=dict(events=dict(count))
        except Exception as e:coverage[h]=dict(error=type(e).__name__)
    coins={a.lower() for rows in sdk["mode"].values() for p in rows for a in (p["address"],)}
    coins.update(sdk["sdk_legacy"])
    for s in recent:coins.update(s["key"][:2])
    markets=[];ignored=[];seen=set()
    # До двух дополнительных уровней для creator→ZORA и его отдельной заявки.
    for _ in range(3):
        todo=sorted(coins-seen)
        if not todo:break
        seen.update(todo)
        for start in range(0,len(todo),15):
            group=todo[start:start+15]
            req=[(dest,sig,args) for a in group for dest,sig,args in ((a,"getPoolKey()",()),(a,"poolAddress()",()),(a,"currency()",()),(FACTORY,"getVersionForDeployedCoin(address)",(a,)),(a,"contractVersion()",()))]
            out=multi(r,req)
            for pos,a in enumerate(group):
                key_raw,v3_raw,currency_raw,reg_raw,version_raw=out[pos*5:pos*5+5]
                if not currency_raw or len(currency_raw)!=32 or not version_raw:ignored.append(a);continue
                cash,=decode(["address"],currency_raw);version,=decode(["string"],version_raw);reg,=decode(["uint8"],reg_raw)
                if a==cash:continue
                if key_raw and len(key_raw)==160:
                    key,=decode([KEY],key_raw);pid=keccak(key_raw)
                    if set(key[:2])!={a,cash} or key[4] not in hooks:ignored.append(a);continue
                    roles_raw,slot,liq=multi(r,[(key[4],"getPoolCoinByHash(bytes32)",(pid,)),(VIEW,"getSlot0(bytes32)",(pid,)),(VIEW,"getLiquidity(bytes32)",(pid,))])
                    if not slot:continue
                    sqrt,tick,protocol,lp=decode(["uint160","int24","uint24","uint24"],slot)
                    if not sqrt:continue
                    registered=None
                    if len(roles_raw)>=128 and int.from_bytes(roles_raw[:32])==32 and int.from_bytes(roles_raw[64:96])==64:
                        n=int.from_bytes(roles_raw[96:128])
                        if len(roles_raw)==128+n*96:
                            (pool_coin,positions),=decode(["(address,(int24,int24,uint128)[])"],roles_raw)
                            registered={"coin":pool_coin,"positions":len(positions)}
                    markets.append(dict(token=a,counter=cash,version=version,config=reg,key=key,id="0x"+pid.hex(),
                        hook_market=registered,sqrt=str(sqrt),tick=tick,protocol=protocol,lp=lp,liquidity=str(decode(["uint128"],liq)[0])))
                    coins.add(cash)
                elif v3_raw and len(v3_raw)==32:
                    pool,=decode(["address"],v3_raw)
                    if int(pool,16)==0:continue
                    t0,t1,fee,factory,liq=r.many([(pool,"token0()",(),"address"),(pool,"token1()",(),"address"),
                        (pool,"fee()",(),"uint24"),(pool,"factory()",(),"address"),(pool,"liquidity()",(),"uint128")])
                    if set((t0,t1))!={a,cash} or factory!=UNI3:ignored.append(a);continue
                    [official]=r.many([(factory,"getPool(address,address,uint24)",(t0,t1,fee),"address")])
                    if official!=pool:ignored.append(a);continue
                    markets.append(dict(token=a,counter=cash,version=version,config=reg,v3=pool,factory=factory,fee=fee,liquidity=str(liq),token_hash=r.code_hash(a)))
                    coins.add(cash)
                else:ignored.append(a)
    return dict(tag=r.tag,height=r.height,sources=["ourzora/zora-protocol@32c2931a444b4405b107a49016dff0059b05b06f","https://api-sdk.zora.engineering/explore"],factory=FACTORY,
        factory_version=factory_version,registry=REGISTRY,router=ROUTER,manager=MANAGER,roles=roles,active_content=content,active_creator=creator,
        registry_nonce=int(r.rpc("eth_getTransactionCount",[FACTORY,r.tag]),16),coverage=[r.height-2047,r.height],activity=coverage,swaps=recent,
        markets=markets,ignored=sorted(set(ignored)))

if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("output",type=Path)
    p.parse_args().output.write_text(json.dumps(capture(),separators=(",",":")),encoding="utf8")

