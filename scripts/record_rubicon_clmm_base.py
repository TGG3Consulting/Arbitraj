"""Проверяет рынки Rubicon Base из публичного реестра по одному состоянию сети."""
import argparse
import json
from pathlib import Path

from eth_abi import decode
from eth_utils import keccak

from record_balancer_markets import Capture
from record_pool_executor_controls import record as execute
from record_rubicon_clmm import DOC, FOLDER, ZERO, deployments, fetch, pool_address
from record_woo_base import rpc

TICKERS = "https://coingecko.rubicon.finance/8453/tickers"
WETH = "0x4200000000000000000000000000000000000006"


def collect():
    s = json.loads((FOLDER / "base.json").read_bytes())
    ref = json.loads((FOLDER / "rubicon_clmm_ethereum.json").read_bytes())
    init = bytes.fromhex(ref["creation_hash"][2:])
    d = deployments("base")
    factory, lens = d["RubiconV3Factory"], d["TickLens"]
    c = Capture(s)
    pairs = sorted({tuple(sorted([r["base_currency"].lower(), r["target_currency"].lower()]))
                    for r in fetch(TICKERS).json() if r["chain"] == 8453})
    tiers = {}
    for fee in sorted(map(int,ref["tiers"])):
        spacing, = c.one(factory,"feeAmountTickSpacing(uint24)",["uint24"],[fee],["int24"])
        if spacing:
            tiers[str(fee)] = spacing
    queries = [(pair,fee) for pair in pairs for fee in map(int, tiers)]
    pools = []
    for i in range(0, len(queries), 8):
        group = queries[i:i+8]
        results = c.batch([(factory,"getPool(address,address,uint24)",["address","address","uint24"],
                            [*pair,fee]) for pair,fee in group])
        for (tokens, fee), raw in zip(group,results,strict=True):
            if raw is None:
                raise ValueError("Нет ответа getPool")
            pool, = decode(["address"],raw)
            if pool == ZERO:
                continue
            if pool != pool_address(factory,tokens,fee,init):
                raise ValueError(f"Адрес {pool} не соответствует байткоду создания")
            pools.append(dict(pool=pool,tokens=list(tokens),fee=fee,spacing=tiers[str(fee)]))
    for i in range(0,len(pools),8):
        group=pools[i:i+8]
        raw=c.batch([(p["pool"],"liquidity()",[],[]) for p in group])
        for p,answer in zip(group,raw,strict=True):
            if answer is None: raise ValueError("Нет ответа liquidity")
            p["liquidity"]=str(decode(["uint128"],answer)[0])
    official = TICKERS
    chosen=[p for p in pools if int(p["liquidity"]) and WETH in p["tokens"]]
    if not chosen:
        raise ValueError("Отсутствует ликвидный тестовый рынок с WETH")
    priority=["0xb3836098d1e94ec651d74d053d4a0813316b2a2f","0x833589fcd6edb6e08f4c7c32d4f71b54bda02913","0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf"]
    chosen.sort(key=lambda p:next((i for i,t in enumerate(priority) if t in p["tokens"]),len(priority)))
    virtual=json.loads((FOLDER/"verified_v3_base.json").read_bytes())
    virtual_hash=next(iter(virtual.values()))["pool"]["execution"]["virtual_quoter_keccak"]
    targets=[]
    for p in chosen[:3]:
        side=p["tokens"].index(WETH)
        targets.append(dict(idx=len(targets),pool=p["pool"],tokens=p["tokens"],
                            quotes=[dict(input=str(a), side=side) for a in (10**13,10**15)]))
    witness=execute(dict(kind="v3",network="base",hash=s["selector"]["blockHash"],block=s["block"],
                         contracts={"factory":factory},quoter_runtime_keccak=virtual_hash,pools=targets))
    if not all(len(p["cases"])==2 and all("sold" in t for t in p["cases"]) for p in witness["pools"]):
        raise ValueError(f"Граничные обмены провалились: {[(p['pool'],[t.get('reason') for t in p['cases']]) for p in witness['pools']]}")
    sample=chosen[0]
    slot=c.one(sample["pool"],"slot0()",returns=["uint160","int24","uint16","uint16","uint16","uint8","bool"])
    word=slot[1]//sample["spacing"]//256
    ticks,=c.one(lens,"getPopulatedTicksInWord(address,int16)",["address","int16"],[sample["pool"],word],["(int24,int128,uint128)[]"])
    hashes={}
    for addr in (factory,lens,d["QuoterV2"]):
        code=bytes.fromhex(rpc(s,"eth_getCode",[addr,s["selector"]])[2:])
        if not code:raise ValueError("Нет контракта " + addr)
        hashes[addr]={"bytes":len(code),"keccak256":"0x"+keccak(code).hex()}
    return dict(chain_id=8453,selector=s["selector"],deployment_source=DOC,ticker_source=official,
                registry_pairs=len(pairs),tiers=tiers,creation_hash=ref["creation_hash"],factory=factory,
                tick_lens=lens,quoter=d["QuoterV2"],contracts=hashes,pools=pools,execution=witness,
                lens_sample={"pool":sample["pool"],"word":word,"ticks":[[t,str(n),str(g)] for t,n,g in ticks]})


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    dest=parser.parse_args().output
    dest.write_bytes((json.dumps(collect(),ensure_ascii=False,indent=2)+"\n").encode("utf8"))
