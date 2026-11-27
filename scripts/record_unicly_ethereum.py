"""Unicly Ethereum. Проверка байткода фабрики, 91 пары, двух init-хешей и обычного платежа Router/V5."""
import argparse
import json
from pathlib import Path

import requests
from eth_abi import decode
from eth_utils import keccak
from record_balancer_markets import Capture
from record_base_classic_directory import optional
from record_base_slipstream_executor import ROOT, rpc
from record_diamondswap_base import direct_wallet
from record_ethereum_v2_factories import WETH, digest, raw, registry
from record_ethereum_x7 import ENDPOINTS, ROOT_HASH, verified

FACTORY="0xbacc776b231c571a7e6ab7bc2c8a099e07153377"
ROUTER="0xe6e90bc9f3b95cdb69f48c7bfdd0ede1386b135a"
SDK_INIT="0xbfcc62b757e2ca00eee45a5dd8129354c7b4f88da9cbdfe4da3b2a55bf698633"
SDK="https://github.com/0xLeia/unicly-sdk/blob/81a2f6c0833b8325343a432bbb309198e39b4af0/src/constants.ts"
CORE="https://github.com/uniclyNFT/unicly-core/tree/0bc5a62a311bfdba1521b7dd0e59dd1a9f633956/contracts/UnicSwap"
LLAMA="https://github.com/DefiLlama/dimension-adapters/blob/be9967c3f0d3ccd06c871bab0d16beb3304de719/factory/uniV2.ts#L502-L504"
V2="https://medium.com/unicly/unicly-v2-launch-airdrop-announcement-fcfd1beed6dc"
DATA=ROOT/"tests/data/first_run_dex_2026-11-26"

def directory(c,header):
    first,=c.one(FACTORY,"allPairs(uint256)",["uint256"],[0],["address"])
    roles={}
    codes={}
    records={}
    for role,a in (("Factory",FACTORY),("Pair",first),("Router02",ROUTER)):
        url,record,code=verified(c,a,"UnicSwapV2"+role)
        roles[role]=dict(address=a,runtime=digest(code),bytes=len(code),source=url)
        codes[role]=code
        records[role]=record
    init_code=raw(records["Pair"]["creationBytecode"]["onchainBytecode"])
    offset=codes["Factory"].find(init_code)
    init=digest(init_code)
    if offset<0 or codes["Factory"].count(init_code)!=1 or init==SDK_INIT:
        raise ValueError("не подтверждён фактический код создания пары")
    def source(role):
        return "".join("".join(s["content"].split()) for s in records[role]["sources"].values())
    if not all(x in source("Pair") for x in ("balance0.mul(1000).sub(amount0In.mul(3))","balance1.mul(1000).sub(amount1In.mul(3))",
                   "balance0Adjusted.mul(balance1Adjusted)>=uint(_reserve0).mul(_reserve1).mul(1000**2)")):
        raise ValueError("изменился инвариант пары")
    if (init[2:] not in source("Router02") or "balanceOf(to).sub(balanceBefore)>=amountOutMin" not in source("Router02")
            or c.one(ROUTER,"factory()",returns=["address"])!=(FACTORY,) or c.one(ROUTER,"WETH()",returns=["address"])!=(WETH,)):
        raise ValueError("фабрика, обёртка или минимальная выплата роутера изменились")
    markets=registry(c,FACTORY,init)
    nonce=int(rpc(c.endpoint,"eth_getTransactionCount",[FACTORY,c.selector]),16)
    if nonce!=len(markets)+1 or markets[0]["pool"]!=first:
        raise ValueError("неполный реестр")
    vals=iter(optional(c,[(t,"balanceOf(address)",["address"],[r["pool"]]) for r in markets for t in r["tokens"]]))
    for r in markets:
        r["balances"]=[str(int.from_bytes(v,"big")) if v is not None else None for v in (next(vals),next(vals))]
        x,y=r["tokens"]
        guessed="0x"+keccak(b"\xff"+raw(FACTORY)+keccak(raw(x)+raw(y))+raw(SDK_INIT))[-20:].hex()
        if guessed==r["pool"]:raise ValueError("старый init дал реальный рынок")
        r["sdk_prediction"]=guessed
    return dict(chain_id=1,height=int(header["number"],16),selector=c.selector,state_root=ROOT_HASH,roles=roles,
                pair_init=init,pair_creation_offset=offset,pair_creation_size=len(init_code),sdk_init=SDK_INIT,nonce=nonce,
                sources=[CORE,SDK,LLAMA,V2],markets=markets)

def wallets(c,d,header):
    markets=[m for m in d["markets"] if WETH in m["tokens"] and min(map(int,m["reserves"]))>0]
    markets.sort(key=lambda m:-int(m["reserves"][m["tokens"].index(WETH)]))
    checked=[];unavailable=[]
    for m in markets:
        if m["reserves"]!=m["balances"]:
            unavailable.append(dict(pool=m["pool"],reason="остатки отличаются от резервов"))
            continue
        reserve=int(m["reserves"][m["tokens"].index(WETH)])
        sizes=sorted({min(10**15,reserve//10000),min(10**16,reserve//500)}-{0})
        if len(sizes)!=2:continue
        try:
            legs=direct_wallet(c,m,sizes,int(header["timestamp"],16),3000,ROUTER,wrapped=WETH)
        except (ValueError,KeyError) as error:
            unavailable.append(dict(pool=m["pool"],reason=str(error)[:220]));continue
        checked.append(dict(pool=m["pool"],rounds=legs))
        if len(checked)==4:break
    if not checked:raise ValueError(("не получена выплата кошелька",unavailable[:5]))
    return dict(wallets=checked,unavailable=unavailable)

def capture(endpoint,stage):
    snapshot=json.loads((DATA/"ethereum.json").read_bytes())
    c=Capture(dict(snapshot,endpoint=endpoint))
    header=rpc(endpoint,"eth_getBlockByHash",[c.selector["blockHash"],False])
    if header["stateRoot"]!=ROOT_HASH or header["hash"]!=c.selector["blockHash"] or rpc(endpoint,"eth_chainId",[])!="0x1":
        raise ValueError("иная сеть или исходный блок")
    d=directory(c,header)
    if stage=="wallet":d.update(wallets(c,d,header))
    return d

if __name__=="__main__":
    cli=argparse.ArgumentParser(description=__doc__)
    cli.add_argument("stage",choices=["registry","wallet"])
    cli.add_argument("output",type=Path)
    args=cli.parse_args()
    errors=[]
    for endpoint in ENDPOINTS:
        try:
            args.output.write_text(json.dumps(capture(endpoint,args.stage),separators=(",",":")),encoding="utf8")
            break
        except (requests.RequestException,ValueError,RuntimeError,KeyError) as err:
            errors.append((endpoint,str(err)[:300]))
    else:raise ValueError(errors)
