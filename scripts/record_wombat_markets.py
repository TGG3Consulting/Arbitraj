"""Проверяет документированные Wombat pools, реализации, прямые котировки и симуляции swap."""
import re
import sys
import time
from functools import cache

from bs4 import BeautifulSoup
from eth_utils import keccak
import requests

from record_balancer_deployments import ROOT, read, write
from record_curve_ng_deployments import code
from record_woo_base import FOLDER, SLOT, rpc
from record_base_slipstream_executor import calldata

sys.path.insert(0,str(ROOT/"tools/bsc_record"))
sys.path.insert(0,str(ROOT/"tools/arbitrum_record"))
import wombat
import wombat_calls as wc

REFS = {"HighCovRatioFeePoolV2":"0x44A0e2AC2c99D5cB2910ec1784DD158b7fa3A642",
        "DynamicPoolV2":"0x3DbF3e359a570D46CF5a663AC92DC7Cae77110FB"}
HOLDERS = {"0x3231cb76718cdef2155fc47b5286d82e6eda273f":"0x79c59db40fb74f76fc638286602b7791872e6aa0",
           "0x0e573ce2736dd9637a0b21058352e1667925c7a8":"0xed9ab5bc05a0152b9dc2f7902e8369af7bc18771"}


class Batch(wombat.Batch):
    def run(self, reqs, step=3):
        result=[]
        for offset in range(0,len(reqs),step):
            for delay in (1,2,4,8,16,None):
                try:
                    result.extend(super().run(reqs[offset:offset+step],step))
                    break
                except Exception as error:
                    if delay is None or "429" not in str(error):
                        raise
                    time.sleep(delay)
        return result


class Sim(wc.WSim):
    def responses(self,body):
        for delay in (1,2,4,8,16,32,None):
            try:
                replies=super().responses(body)
                if "rate limit" in str(replies).lower():
                    raise ValueError("429")
                return replies
            except Exception as error:
                if delay is None or "429" not in str(error):
                    raise
                time.sleep(delay)

    def run(self):
        return super().run(batch_size=3)


@cache
def artifact(chain, address):
    url=(f"https://sourcify.dev/server/v2/contract/{chain}/{address}?fields=compilation,sources,"
         "runtimeBytecode.onchainBytecode,runtimeBytecode.linkReferences,storageLayout")
    r=requests.get(url,timeout=25)
    r.raise_for_status()
    return url,r.json()


def functions(source):
    """Сигнатура и тело каждой Solidity-функции (комментарии удалены)."""
    clean=re.sub(r"/\*.*?\*/|//[^\n]*","",source,flags=re.S)
    out={}
    for m in re.finditer(r"\bfunction\s+\w+\s*\([^)]*\)[^{;]*\{",clean):
        pos=m.end()
        depth=1
        while depth and pos<len(clean):
            depth += (clean[pos]=="{") - (clean[pos]=="}")
            pos+=1
        if depth:
            raise ValueError("незакрытое тело функции")
        out[m.group()]=clean[m.start():pos]
    return out


def implementation(e, address):
    url,j=artifact(e["chain_id"],address)
    name=j["compilation"]["name"]
    if name not in REFS or j["runtimeMatch"]!="exact_match":
        raise ValueError(f"неизвестная реализация {name} {address}")
    expected="0x"+keccak(bytes.fromhex(j["runtimeBytecode"]["onchainBytecode"][2:])).hex()
    if code(e,address)!=expected or j["runtimeBytecode"]["linkReferences"]:
        raise ValueError("код реализации расходится с Sourcify либо ссылается на внешнюю библиотеку")
    layout=[(r["slot"],r["offset"],r["type"]) for r in j["storageLayout"]["storage"] if r["label"]=="_pausedAssets"]
    if layout!=[("201",0,"t_mapping(t_address,t_bool)")]:
        raise ValueError("слот паузы актива другой")
    ref_url,ref=artifact(42161,REFS[name])
    def sources(x):
        return {p.rsplit("/",1)[-1]:v["content"] for p,v in x["sources"].items()}
    actual,base=sources(j),sources(ref)
    # Совпадение всех выбранных файлов расчёта и исходных функций проверенных математических библиотек.
    files=["CoreV2.sol","PoolV2.sol","PausableAssets.sol","HighCovRatioFeePoolV2.sol"]
    if name=="DynamicPoolV2":
        files.append("DynamicPoolV2.sol")
    for p in files:
        if actual[p]!=base[p]:
            raise ValueError(f"отличается {name}/{p}")
    library_functions={}
    for p in ["DSMath.sol","SignedSafeMath.sol"]:
        before,after=functions(base[p]),functions(actual[p])
        if not before or any(after.get(sig)!=body for sig,body in before.items()):
            raise ValueError(f"отличаются ранее используемые методы {p}")
        library_functions[p]=len(before)
    return dict(kind=name,verified=url,reference=ref_url,runtime=expected,paused_assets_slot=201,
                source_keccak={p:keccak(text=actual[p]).hex() for p in files},matched_functions=library_functions)


def funded_swaps(e,m):
    """Независимый funded EOA, если исторический recorder не нашёл слоты прокси токена."""
    rows=[]
    for s in m.get("swap_sim",[]):
        source,target=s["from"],s["to"]
        if s.get("override_ok") or source not in HOLDERS:
            continue
        sender=HOLDERS[source]
        dx=int(s["dx"])
        pool=m["pool"]
        q=calldata("quotePotentialSwap(address,address,int256)",["address","address","int256"],[source,target,dx])
        app=calldata("approve(address,uint256)",["address","uint256"],[pool,dx])
        swap=calldata("swap(address,address,uint256,uint256,address,uint256)",
                      ["address","address","uint256","uint256","address","uint256"],[source,target,dx,1,sender,2**256-1])
        bal=calldata("balanceOf(address)",["address"],[sender])
        calls=[{"from":sender,"to":to,"data":d} for to,d in [(source,app),(pool,q),(source,bal),(target,bal),
                                                             (pool,swap),(source,bal),(target,bal)]]
        block,=rpc(e,"eth_simulateV1",[{"blockStateCalls":[{"calls":calls}]},e["selector"]])
        if block["parentHash"]!=e["selector"]["blockHash"] or len(block["calls"])!=7:
            raise ValueError("неожиданная симуляция")
        result=block["calls"]
        if any(result[i]["status"]!="0x1" for i in (0,2,3,5,6)):
            raise ValueError("не прошли условия подготовки и измерения")
        amount=lambda i:int(result[i]["returnData"],16)
        ok=result[4]["status"]=="0x1"
        paid,got=amount(2)-amount(5),amount(6)-amount(3)
        if (ok and (paid!=dx or got<=0 or got!=int(result[4]["returnData"][2:66],16))) or (not ok and (paid or got)):
            raise ValueError("балансы обмена расходятся")
        rows.append(dict(i=s["i"],j=s["j"],dx=s["dx"],holder=sender,paid=str(paid),received=str(got),
                         ok=ok,parent_hash=block["parentHash"],calls=calls,results=result))
    return rows


def collect(network):
    e=read(FOLDER/f"{network}.json")
    route=e.copy()
    if network=="ethereum":
        route["endpoint"]="https://mainnet.gateway.tenderly.co"
    url=f"https://docs.wombat.exchange/developers/contracts/{network}.md"
    response=requests.get(url,timeout=15)
    response.raise_for_status()
    page=BeautifulSoup(response.text,"html.parser")
    doc={}
    for row in page.find_all("tr"):
        cols=row.find_all("td")
        if len(cols)==2 and "pool" in cols[0].get_text().lower():
            doc[cols[1].get_text().strip().lower()]=cols[0].get_text().strip()
    if not doc:
        raise ValueError("в официальной документации не найдены пулы")
    b=Batch(route["endpoint"])
    tag=e["selector"]
    header=rpc(e,"eth_getBlockByHash",[tag["blockHash"],False])
    sim=Sim(route["endpoint"],tag)
    if not sim.empty():
        raise ValueError("условия симуляции не выполнены")
    entries,impls,tokens={}, {}, {}
    for pool,name in doc.items():
        slot=rpc(e,"eth_getStorageAt",[pool,SLOT,tag])
        imp="0x"+slot[-40:]
        entries[pool]=dict(name=name,implementation=imp)
        if name=="Cross Chain Pool":
            entries[pool]["status"]="CrossChainPool/CoreV3"
            continue
        if imp not in impls:
            impls[imp]=implementation(e,imp)
        code(e,pool)
        raw=rpc(e,"eth_call",[{"to":pool,"data":"0x"+keccak(text="getTokens()")[:4].hex()},tag])
        tokens[pool]=wc.tokens_of(raw)
    checked=[(doc[p],p) for p in tokens]
    _,markets=wombat.state_phase(b,sim,checked,tokens,"0.0001,0.01",header,block_selector=tag)
    kept=[]
    for m in markets:
        pool=m["pool"]
        usable={}
        m["funded_swaps"]=funded_swaps(e,m)
        for trade in m["funded_swaps"]:
            if trade["ok"]:
                usable.setdefault((trade["i"],trade["j"]),set()).add(int(trade["dx"]))
        for s in m.get("swap_sim",[]):
            if s.get("ok") and s.get("override_ok") and int(s["result"][2:66],16)>0:
                usable.setdefault((s["i"],s["j"]),set()).add(int(s["dx"]))
        complete=any(len(sizes)>=2 and len(usable.get((j,i),set()))>=2 for (i,j),sizes in usable.items())
        if complete:
            status="swap"
        elif any(len(sizes)>=2 for sizes in usable.values()):
            status="directional_swap"
        else:
            status="insufficient_simulated_sides"
        entries[pool]["status"]=status
        if status in ("swap","directional_swap"):
            kept.append(pool)
    write(FOLDER/f"wombat_{network}.json",dict(url=url,chain_id=e["chain_id"],selector=tag,endpoint=route["endpoint"],
          documented=entries,implementations=impls,pools=markets))
    write(FOLDER/f"{network}.json",e)
    print(network,[(r["name"],r["status"]) for r in entries.values()],flush=True)
    if not kept:
        return None
    def group(name):
        return sorted({entries[p]["implementation"] for p in kept if impls[entries[p]["implementation"]]["kind"]==name})
    settings={"dex.kind":"wombat","dex.stable_pools":kept,"dex.wombat_implementations":group("HighCovRatioFeePoolV2")}
    dynamic=group("DynamicPoolV2")
    if dynamic:
        settings["dex.wombat_dynamic_implementations"]=dynamic
    return dict(network=network,chain_id=e["chain_id"],id="wombat-exchange",source=url,settings=settings)


if __name__=="__main__":
    rows=[p for n in ("base","ethereum") if (p:=collect(n))]
    path=ROOT/"config/first_run_dex.json"
    write(path,[r for r in read(path) if r["network"] not in ("base","ethereum") or r["id"]!="wombat-exchange"]+rows)
