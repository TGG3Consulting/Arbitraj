"""Проверяет ранние Curve Plain2/3/4 по решениям 472 и 481 на Base и Ethereum.

Для ABI из Curve SDK сверяет код с эталоном Arbitrum или исходник функций get_dy/exchange
и их зависимостей вместе с числовыми константами. Обе сети проверяются собственными RPC.
"""
from concurrent.futures import ThreadPoolExecutor
from functools import lru_cache
import hashlib
import re

from eth_utils import keccak
import requests

from record_balancer_deployments import ROOT, read, write
from record_balancer_markets import Capture
from record_curve_ng_deployments import SDK, code, source

SDK_SOURCE = f"https://raw.githubusercontent.com/curvefi/curve-js/{SDK}/src/constants/factory/stable.ts"


@lru_cache
def verified(chain, addr):
    url=f"https://sourcify.dev/server/v2/contract/{chain}/{addr}?fields=sources,compilation,runtimeBytecode.onchainBytecode"
    r=requests.get(url, timeout=22)
    if r.status_code==404:
        return None
    r.raise_for_status()
    d=r.json()
    if d["runtimeMatch"] not in ("match","exact_match") or d["compilation"]["language"].lower()!="vyper":
        return None
    source_file=d["compilation"]["fullyQualifiedName"].rsplit(":",1)[0]
    return (url, "0x"+keccak(bytes.fromhex(d["runtimeBytecode"]["onchainBytecode"][2:])).hex(),
            d["sources"][source_file]["content"])


def formula(src):
    """Одинаковый текст get_dy и exchange вместе с вызываемыми функциями, константами и отступами."""
    constants=dict(re.findall(r"^(\w+): constant\(\w+\) = ([^#\n]+)",src,flags=re.M))
    funcs={m[1]:m[0] for m in re.finditer(r"^def (\w+)\(.*?(?=^@|^def |\Z)",src,flags=re.S|re.M)}
    if "N_COINS_128" in constants and constants["N_COINS_128"].strip()!=constants["N_COINS"].strip():
        raise ValueError("разные размерности")
    def collect(name, out):
        if name in out or name not in funcs:
            return
        body=re.sub(r'""".*?"""','',funcs[name],flags=re.S)
        lines=[l.split("#",1)[0].rstrip() for l in body.splitlines()]
        out[name]="\n".join(l for l in lines if l.strip()).replace("N_COINS_128","N_COINS")
        for sub in re.findall(r"self\.(\w+)\(",out[name]):
            collect(sub,out)
    out={}
    for name in ("get_dy","exchange","A_precise"):
        collect(name,out)
    if not {"get_dy","exchange","A_precise","get_D","get_y"}.issubset(out):
        return None
    combined="\n".join(out.values())
    for key,value in constants.items():
        if re.search(r"\b"+key+r"\b",combined):
            out[key]=value.strip()
    return hashlib.sha256("\n".join(k+"\n"+v for k,v in sorted(out.items())).encode()).hexdigest()


def sdk_templates(text,network):
    region=text.split("implementationABIDict"+network.title()+":")[1].split("\n})")[0]
    return re.findall(r'"(0x[\da-fA-F]{40})":\s*(Plain\w+ABI)',region)


def capture(net, factory, allowed, e):
    c=Capture(e)
    url=f"https://api.curve.finance/v1/getPools/{net}/factory"
    api=requests.get(url,timeout=18)
    api.raise_for_status()
    rows=sorted(api.json()["data"]["poolData"], key=lambda p:float(p.get("usdTotal") or 0),reverse=True)
    proofs=[]
    for row in rows:
        candidate=row.get("implementationAddress","").lower()
        if candidate not in allowed:
            continue
        p=row["address"]
        impl,=c.one(factory,"get_implementation_address(address)",["address"],[p],["address"])
        if impl!=candidate:
            continue
        # Для пула фабрики сначала сверяем число монет с проверенным шаблоном. В API могут быть метапулы.
        n=int(allowed[impl][5])
        coins=[c.one(p,"coins(uint256)",["uint256"],[i],["address"])[0] for i in range(n)]
        balances=[c.one(p,"balances(uint256)",["uint256"],[i],["uint256"])[0] for i in range(n)]
        if not min(balances) or any(int(a,16)==0 for a in coins):
            continue
        amp,=c.one(p,"A_precise()",returns=["uint256"])
        fee,=c.one(p,"fee()",returns=["uint256"])
        quotes=[]
        for i,j in [(0,1),(1,0)]:
            for divisor in (100000,1000):
                dx=balances[i]//divisor
                raw,=c.batch([(p,"get_dy(int128,int128,uint256)",["int128","int128","uint256"],[i,j,dx])])
                if raw and int.from_bytes(raw,"big"):
                    quotes.append(dict(i=i,j=j,dx=str(dx),dy=str(int.from_bytes(raw,"big"))))
        if len(quotes)==4:
            dec=[]
            for a in coins:
                val=18 if a=="0x"+"ee"*20 else c.one(a,"decimals()",returns=["uint8"])[0]
                dec.append(val)
            proofs.append(dict(pool=p,implementation=impl,coins=coins,decimals=dec,balances=list(map(str,balances)),a=str(amp),fee=str(fee),quotes=quotes))
        if len(proofs)==4:
            break
    return dict(endpoint=e["endpoint"],selector=e["selector"],candidate_source=url,pools=proofs,calls=c.calls)


def main():
    r=requests.get(SDK_SOURCE,timeout=18)
    r.raise_for_status()
    sdk=r.text
    aliases=source("stable-ng")
    doc=(ROOT/"docs/notes/arbitrum/dex-settings-arbitrum.md").read_text("utf8")
    audited=next(l for l in doc.splitlines() if l.startswith("| dex.curve_plain_implementations |"))
    ref=[(a,label) for a,label in sdk_templates(sdk,"arbitrum") if a.lower() in audited.lower()]
    archive = "https://arbitrum-one-rpc.publicnode.com"
    block=requests.post(archive,json=dict(id=1,jsonrpc="2.0",method="eth_getBlockByNumber",params=["latest",False]),timeout=20).json()["result"]
    reference=dict(endpoint=archive,selector={"blockHash":block["hash"],"requireCanonical":True},contracts={})
    for addr,_ in ref:
        code(reference,addr)
    with ThreadPoolExecutor(4) as workers:
        refs=list(workers.map(lambda v:verified(42161,v[0]),ref))
    for net in ("base","ethereum"):
        folder=ROOT/"tests/data/first_run_dex_2026-11-26"
        e=read(folder/f"{net}.json")
        scope=aliases.split(f"const ALIASES_{net.upper()} = lowerCaseValues(")[1].split("});")[0]
        factory=re.search('"factory"'+r":\s*'([^']+)'",scope)[1]
        code(e,factory)
        templates=sdk_templates(sdk,net)
        with ThreadPoolExecutor(4) as workers:
            vs=list(workers.map(lambda v:verified(e["chain_id"],v[0]),templates))
        accepted={}
        checks=[]
        for (address,label),v in zip(templates,vs,strict=True):
            runtime=code(e,address)
            item=dict(address=address,sdk_abi=label,runtime=runtime,status="unproven")
            checks.append(item)
            if v and v[1]!=runtime:
                continue
            for (a,kind),canonical in zip(ref,refs,strict=True):
                if kind!=label:
                    continue
                expected=reference["contracts"][a.lower()]["keccak256"]
                same=runtime==expected
                digest=formula(v[2]) if v else None
                if same or (digest and canonical and canonical[1]==expected and digest==formula(canonical[2])):
                    item.update(status="same_bytecode" if same else "same_formula",source=v[0] if v else SDK_SOURCE,
                                reference=canonical[0] if canonical else SDK_SOURCE,
                                reference_address=a,reference_runtime=expected,formula_sha256=digest)
                    accepted[address.lower()]=label
                    break
        proof=capture(net,factory,accepted,e)
        proof.update(chain_id=e["chain_id"],factory=factory,templates=checks,arbitrum_reference=reference)
        if not proof["pools"]:
            raise ValueError(f"{net}: нет действующего контрольного пула")
        write(folder/f"curve_plain_{net}.json",proof)
        write(folder/f"{net}.json",e)
        path=ROOT/"config/first_run_dex.json"
        profiles=read(path)
        parent="curve" if net=="ethereum" else "curve-base"
        key=parent+"-plain"
        parent_row=next(r for r in profiles if r["network"]==net and r["id"]==parent)
        children=parent_row["settings"]["dex.sub_dex"]
        if key not in children:
            children.append(key)
        row=dict(chain_id=e["chain_id"],network=net,id=key,source=SDK_SOURCE,settings={
                 "dex.kind":"curve_stableswap_plain","dex.factory":factory,"dex.stable_pools":[],
                 "dex.curve_plain_implementations":list(accepted)})
        write(path,[r for r in profiles if r["network"]!=net or r["id"]!=key]+[row])
        print(net,len(accepted),len(proof["pools"]), flush=True)


if __name__=="__main__":
    main()
