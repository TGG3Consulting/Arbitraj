"""Сверяет фабрики DODO V2, реестр, состояние PMM, комиссии и котировки на каноническом блоке."""
import itertools
from pathlib import Path

from bs4 import BeautifulSoup
from eth_abi import decode, encode
from eth_utils import keccak
import requests

from record_balancer_deployments import ROOT, read, write
from record_balancer_markets import Capture, ZERO
from record_curve_ng_deployments import code

# Ликвидность ищется на реальных контрактных парах. Токены служат аргументами запроса к реестру.
TOKENS = {
    "base": ["0x4200000000000000000000000000000000000006","0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
             "0xd9aaec86b65d86f6a7b5b1b0c42ffa531710b6ca","0x50c5725949a6f0c72e6c4a641f24049a917db0cb"],
    "ethereum": ["0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2","0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
                 "0xdac17f958d2ee523a2206206994597c13d831ec7","0x6b175474e89094c44da98b954eedeac495271d0f"],
}
MULTICALL = "0xca11bde05977b3631167028862be2a173976ca11"


def find_markets(c, factory, tokens):
    """Проверяет реестр в обе стороны и рассматривает пулы с наибольшим произведением резервов своей пары."""
    result = []
    for token0, token1 in itertools.combinations(tokens, 2):
        forward, reverse = c.one(factory,"getDODOPoolBidirection(address,address)",["address","address"],
                                 [token0,token1],["address[]","address[]"])
        pools = list(dict.fromkeys(forward + reverse))
        funded = []
        for start in range(0,len(pools),60):
            part = pools[start:start+60]
            reads = [(a,keccak(text="getPMMStateForCall()")[:4]) for a in part]
            replies, = c.one(MULTICALL,"tryAggregate(bool,(address,bytes)[])",["bool","(address,bytes)[]"],
                             [False, reads],["(bool,bytes)[]"])
            for a,(ok,data) in zip(part,replies,strict=True):
                if ok and len(data)==224:
                    s = decode(["uint256"] * 7,data)
                    if s[2] and s[3] and s[6]<3:
                        funded.append((s[2]*s[3], a, s))
        for _,pool,state in sorted(funded,reverse=True)[:6]:
            entry = dict(pool=pool, state=[str(v) for v in state], quotes=[])
            result.append(entry)
            try:
                a, = c.one(pool,"_BASE_TOKEN_()",returns=["address"])
                b, = c.one(pool,"_QUOTE_TOKEN_()",returns=["address"])
                if ((pool in forward and (a,b) != (token0,token1)) or
                    (pool in reverse and (a,b) != (token1,token0))):
                    raise ValueError("ориентация токенов расходится с реестром")
                fees = c.one(pool,"getUserFeeRate(address)",["address"],[ZERO],["uint256","uint256"])
                entry.update(base=a,quote=b,fees=list(map(str,fees)))
                for method,balance in [("querySellBase",state[2]),("querySellQuote",state[3])]:
                    for divisor in (100000,1000):
                        amount = balance//divisor
                        if not amount:
                            raise ValueError("резерв меньше объёма пробы")
                        out, = c.one(pool,method+"(address,uint256)",["address","uint256"],[ZERO,amount],["uint256"])
                        if not out:
                            raise ValueError("нулевой выход")
                        entry["quotes"].append(dict(sell=method,amount_in=str(amount),amount_out=str(out)))
                entry["status"] = "quoted"
            except ValueError as err:
                entry.update(status="unquoted", reason=str(err))
            if sum(r["status"]=="quoted" for r in result) == 2:
                return result
    return result


def main():
    folder = ROOT/"tests/data/first_run_dex_2026-11-26"
    target = ROOT/"config/first_run_dex.json"
    profiles = []
    for network in TOKENS:
        url = "https://docs.dodoex.io/en/developer/contracts/dodo-v1-v2/contracts-address/"+network
        response = requests.get(url, timeout=20)
        response.raise_for_status()
        table = BeautifulSoup(response.text, "html.parser")
        addresses = {}
        for tr in table.select("tr"):
            cells = [td.get_text(strip=True) for td in tr.select("td")]
            if len(cells)==2:
                addresses[cells[0]]=cells[1]
        e = read(folder/f"{network}.json")
        by_kind = {}
        parent = "dodo-pmm-ethereum" if network=="ethereum" else "dodo-amm-dvm"
        for kind in ("DVM","DPP","DSP"):
            factory = addresses[kind+"Factory"]
            code(e,factory)
            c = Capture(e)
            markets = find_markets(c,factory,TOKENS[network])
            by_kind[kind] = dict(factory=factory,pools=markets,calls=c.calls)
            if markets and any(r["status"]=="quoted" for r in markets):
                key = parent if kind=="DVM" else "dodo-amm-"+kind.lower()
                profiles.append(dict(chain_id=e["chain_id"],network=network,id=key,source=url+" · "+kind+"Factory",
                                settings={"dex.kind":"dodo_"+kind.lower(),"dex.factory":factory}))
        if network=="ethereum":
            dvm = next(r for r in profiles if r["id"]==parent and r["network"]==network)
            dvm["settings"]["dex.sub_dex"]=[r["id"] for r in profiles if r["network"]==network and r["id"]!=parent]
        write(folder/f"{network}.json", e)
        write(folder/f"dodo_{network}.json",dict(chain_id=e["chain_id"],selector=e["selector"],endpoint=e["endpoint"],
                                                trader=ZERO,source=url,families=by_kind))
        print(network,[(k,sum(r["status"]=="quoted" for r in f["pools"])) for k,f in by_kind.items()],flush=True)
    ids = {(r["network"],r["id"]) for r in profiles}
    write(target,[r for r in read(target) if (r["network"],r["id"]) not in ids]+profiles)


if __name__=="__main__":
    main()
