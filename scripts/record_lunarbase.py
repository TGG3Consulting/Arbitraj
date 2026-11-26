"""Записывает реальные размещения и две стороны открытого пула LunarBase через eth_call и eth_simulateV1."""
import hashlib
import requests
from eth_utils import keccak

from record_balancer_deployments import ROOT, read, write
from record_base_slipstream_executor import EXEC, calldata
from record_woo_base import SLOT

FOLDER = ROOT / "tests/data/first_run_dex_2026-11-26"
SOURCE = "https://raw.githubusercontent.com/Lunarbase-Lab/LunarBaseIntegrations/2adec126295e866efbf3c1c2be936a792ba1303a/"
LLAMA = "https://raw.githubusercontent.com/DefiLlama/dimension-adapters/be9967c3f0d3ccd06c871bab0d16beb3304de719/dexs/lunarbase/index.ts"
POOLS = ["0x6ccc8223532fff07f47ef4311beb3647326894ab","0x00003bf45ce34bf1bea78669f9a40ee630e11b99",
         "0x0000efc4ec03a7c47d3a38a9be7ff1d52dd01b99","0xcb1c06554772bc855d81a6be648cc599710e1b99"]
ZERO = "0x"+"00"*20
USDC = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
OWNER = "0xb4807865a786e9e9e26e6a9610f2078e7fc507fb"
MULTICALL = "0xca11bde05977b3631167028862be2a173976ca11"


class Capture:
    def __init__(self, chain):
        self.url, self.tag = chain["endpoint"], chain["selector"]
        self.session = requests.Session()

    def batch(self, calls):
        if any(m not in {"eth_getCode","eth_getStorageAt","eth_getBlockByHash","eth_call","eth_simulateV1"} for m,_ in calls):
            raise ValueError("Метод вне перечня чтения")
        body = [dict(jsonrpc="2.0",id=i,method=m,params=p) for i,(m,p) in enumerate(calls)]
        r = self.session.post(self.url,json=body,timeout=35); r.raise_for_status()
        by_id = {row["id"]:row for row in r.json()}
        return [by_id[i] for i in range(len(body))]

    def one(self, method, params):
        return self.batch([(method,params)])[0]["result"]

    def code(self, address):
        bytecode = bytes.fromhex(self.one("eth_getCode",[address,self.tag])[2:])
        return dict(bytes=len(bytecode),keccak256="0x"+keccak(bytecode).hex())

    def market(self, p):
        names = ["X()","Y()","state()","getXReserve()","getYReserve()","isFresh()","paused()","blockDelay()",
                 "blacklistFeeMultiplier()","maxPunishmentX24()","owner()"]
        data = [(sig,{"to":p,"data":calldata(sig)}) for sig in names]
        for account in (OWNER,EXEC):
            data.append(("isWhitelisted(address)",{"to":p,"data":calldata("isWhitelisted(address)",["address"],[account])}))
        raw = self.batch([("eth_call",[q,self.tag]) for _,q in data])
        impl_word = self.one("eth_getStorageAt",[p,SLOT,self.tag]); impl="0x"+impl_word[-40:]
        return dict(address=p,code=self.code(p),implementation_slot=SLOT,implementation_word=impl_word,
                    implementation=impl,implementation_code=self.code(impl) if impl!=ZERO else None,
                    views=[dict(signature=s,request=q,response=r) for (s,q),r in zip(data,raw,strict=True)])

    def simulate(self, entries):
        calls = [{"from":OWNER,"to":to,"data":data,"value":hex(value),"gas":hex(1_000_000)} for _,to,data,value in entries]
        stage, = self.one("eth_simulateV1",[{"blockStateCalls":[{"calls":calls}],"validation":False},self.tag])
        if stage["parentHash"] != self.tag["blockHash"]:
            raise ValueError("Сменился родительский канонический блок")
        return calls, dict(zip((k for k,_,_,_ in entries),stage["calls"],strict=True))

    def measure(self, pool, asset, amount, deadline):
        other = USDC if asset == ZERO else ZERO
        query = calldata("quoteExactIn(address,address,uint256)",["address","address","uint256"],[asset,other,amount])
        quote = ("quote",pool,query,0)
        _, first = self.simulate([quote])
        out = int(first["quote"]["returnData"],16)
        if not out: raise ValueError("Котировка исчерпана")
        def swap(key, minimum, end):
            if asset == ZERO:
                sig = "swapExactInNative(address,address,uint256,uint256)"
                return (key,pool,calldata(sig,["address","address","uint256","uint256"],[other,OWNER,minimum,end]),amount)
            sig = "swapExactIn((address,address,address,uint256,uint256,uint256))"
            return (key,pool,calldata(sig,["(address,address,address,uint256,uint256,uint256)"],[(asset,other,OWNER,amount,minimum,end)]),0)
        def balances(before):
            return [("eth_"+before,MULTICALL,calldata("getEthBalance(address)",["address"],[OWNER]),0),
                    ("usdc_"+before,USDC,calldata("balanceOf(address)",["address"],[OWNER]),0)]
        entries = balances("before")
        if asset != ZERO:
            for n in (0,amount):
                entries.append(("approve_"+str(n),USDC,calldata("approve(address,uint256)",["address","uint256"],[pool,n]),0))
        entries += [quote,swap("expired",out,0),swap("above",out+1,deadline),swap("exact",out,deadline)]+balances("after")
        calls, result = self.simulate(entries)
        for k,r in result.items():
            if r["status"] != ("0x0" if k in ("expired","above") else "0x1"):
                raise ValueError(f"{asset}/{amount} {k}: {r.get('error')}")
        n=lambda k:int(result[k]["returnData"],16)
        eth = n("eth_after")-n("eth_before"); usdc = n("usdc_after")-n("usdc_before")
        if (eth,usdc) != ((-amount,out) if asset==ZERO else (out,-amount)) or n("exact")!=out or n("quote")!=out:
            raise ValueError("Котировка, возврат и фактические дельты расходятся")
        return dict(token_in=asset,token_out=other,amount=str(amount),output=str(out),gas=n_gas(result["exact"]),calls=calls,results=result)


def n_gas(row):
    return int(row["gasUsed"],16)


def capture():
    e=read(FOLDER/"base.json"); c=Capture(e)
    files={name:requests.get(SOURCE+name,timeout=20) for name in ["mainnet/addresses.json","abi/Pool.trading.abi.json"]}
    for res in files.values(): res.raise_for_status()
    primary=files["mainnet/addresses.json"].json()["networks"]["Base"]["pools"]["ETH/USDC"]
    upstream=requests.get(LLAMA,timeout=20); upstream.raise_for_status()
    if primary["proxy"].lower()!=POOLS[2] or any(p not in upstream.text.lower() for p in POOLS):
        raise ValueError("Список размещений изменился")
    head=c.one("eth_getBlockByHash",[c.tag["blockHash"],False]); deadline=int(head["timestamp"],16)+90
    at=[c.market(p) for p in POOLS]
    cases=[c.measure(POOLS[2],token,n,deadline) for token,steps in ((ZERO,(10**14,10**15)),(USDC,(10**6,10**7))) for n in steps]
    return dict(chain_id=8453,selector=c.tag,endpoint=c.url,source=SOURCE,adapter=LLAMA,official=primary,
                sha256={k:hashlib.sha256(v.content).hexdigest() for k,v in files.items()},markets=at,owner=OWNER,cases=cases)


if __name__ == "__main__":
    write(FOLDER/"lunarbase.json",capture())
