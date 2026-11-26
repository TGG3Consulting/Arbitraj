"""Проверяет официальный ElfomoFi, реальные балансы прямого обмена, плательщика и нижнюю границу выплаты."""
import hashlib

import requests
from bs4 import BeautifulSoup
from eth_abi import decode
from eth_utils import keccak

from record_lista_ethereum import Capture, FOLDER, calldata, deployed, read, rpc, write
from record_solidly_v3 import HOLDERS
from record_woo_base import SLOT

DOC = "https://docs.elfomo.fi/integration/"
ROUTER = "0xf0f0f0f0fb0d738452efd03a28e8be14c76d5f73"
SIZES = {"0x4200000000000000000000000000000000000006":(10**14,10**15),
         "0xcbb7c0000ab88b473b1f5afd9ef808440eed33bf":(1000,10000),
         "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913":(10**6,10**7),
         "0xfde4c96c8593536e31f229ea8f37b2ada2699bb2":(10**6,10**7)}


def measure(e, token_in, token_out, amount, vault):
    owner = HOLDERS["base"][token_in]
    call = lambda to, data: {"from":owner,"to":to,"data":data,"gas":hex(1500000)}
    bal = lambda holder: calldata("balanceOf(address)",["address"],[holder])
    allowance = calldata("allowance(address,address)",["address"]*2,[owner,ROUTER])
    approval = lambda n: calldata("approve(address,uint256)",["address","uint256"],[ROUTER,n])
    query = calldata("getAmountOut(address,address,uint256)",["address","address","uint256"],[token_in,token_out,amount])
    swap = lambda n: calldata("swap(address,address,int256,uint256,address,uint256)",
                              ["address","address","int256","uint256","address","uint256"],
                              [token_in,token_out,amount,n,owner,0])
    def simulate(entries):
        calls = [call(to,data) for label,to,data in entries]
        stage, = rpc(e,"eth_simulateV1",[{"blockStateCalls":[{"calls":calls}],"validation":False},e["selector"]])
        if stage["parentHash"] != e["selector"]["blockHash"]:
            raise ValueError("канонический хеш изменился")
        return dict(zip((k for k,_,_ in entries),stage["calls"],strict=True)),calls
    prefix = [("before_in",token_in,bal(owner)),("before_out",token_out,bal(owner)),
              ("vault_in",token_in,bal(vault)),("vault_out",token_out,bal(vault)),
              ("reset",token_in,approval(0)),("approve",token_in,approval(amount)),("quote",ROUTER,query)]
    first,_ = simulate(prefix)
    if first["quote"]["status"]!="0x1":
        return dict(token_in=token_in,token_out=token_out,owner=owner,amount=str(amount),
                    status="quote_reverted",quote=first["quote"],calldata=query)
    out = int(first["quote"]["returnData"],16)
    if not out:
        return dict(token_in=token_in,token_out=token_out,owner=owner,amount=str(amount),
                    status="zero_quote",quote=first["quote"],calldata=query)
    entries = prefix+[("above",ROUTER,swap(out+1)),("exact",ROUTER,swap(out)),("after_in",token_in,bal(owner)),
                      ("after_out",token_out,bal(owner)),("after_vault_in",token_in,bal(vault)),
                      ("after_vault_out",token_out,bal(vault)),("remaining",token_in,allowance)]
    results,calls = simulate(entries)
    if results["exact"]["status"]=="0x0":
        return dict(token_in=token_in,token_out=token_out,owner=owner,amount=str(amount),out=str(out),
                    status="swap_reverted",calls=calls,results=results)
    if any(r["status"]!=("0x0" if k=="above" else "0x1") for k,r in results.items()):
        raise ValueError("неожиданный статус вызова")
    n = lambda k:int(results[k]["returnData"],16)
    if (n("before_in")-n("after_in"),n("after_out")-n("before_out"),n("after_vault_in")-n("vault_in"),
        n("vault_out")-n("after_vault_out"),n("remaining"))!=(amount,out,amount,out,0):
        raise ValueError("разошлись полные дельты кошелька и vault или остаток разрешения")
    if not results["above"]["error"]["data"].startswith("0x"+keccak(text="InsufficientAmount(uint256,uint256)")[:4].hex()):
        raise ValueError("minOut + 1 отклонён по другой причине")
    return dict(token_in=token_in,token_out=token_out,owner=owner,amount=str(amount),out=str(out),
                status="filled",calls=calls,results=results)


def capture():
    e=read(FOLDER/"base.json")
    c=Capture(e)
    page=requests.get(DOC,timeout=20)
    page.raise_for_status()
    page_text=BeautifulSoup(page.content,"html.parser").get_text(" ",strip=True)
    if ROUTER not in page_text.lower() or "0 if not used" not in page_text:
        raise ValueError("официальный адрес или правило выбора partnerId изменились")
    router,url=deployed(e,ROUTER)
    params=requests.get(url,params={"fields":"abi,creationBytecode.transformationValues"},timeout=20).json()
    signature=next(a for a in params["abi"] if a.get("name")=="swap")
    if [x["type"] for x in signature["inputs"]]!=["address","address","int256","uint256","address","uint256"] or signature["outputs"]:
        raise ValueError("ABI swap изменился")
    args=params["creationBytecode"]["transformationValues"]["constructorArguments"]
    pricing,vault=decode(["address","address"],bytes.fromhex(args[2:]))
    impl="0x"+rpc(e,"eth_getStorageAt",[pricing,SLOT,e["selector"]])[-40:]
    code=lambda a: "0x"+keccak(bytes.fromhex(rpc(e,"eth_getCode",[a,e["selector"]])[2:])).hex()
    markets=c.one(ROUTER,"getSupportedPairs()",returns=["(address,address)[]"])[0]
    cases=[measure(e,a,b,n,vault) for x,y in markets if x in SIZES and y in SIZES
           for a,b in ((x,y),(y,x)) for n in SIZES[a]]
    return dict(chain_id=e["chain_id"],selector=e["selector"],endpoint=e["endpoint"],source=DOC,router=ROUTER,
                router_runtime=code(ROUTER),verified=url,source_sha256=hashlib.sha256(
                    router["sources"]["src/ElfomoFi.sol"]["content"].encode()).hexdigest(),
                pricing=pricing,pricing_code=code(pricing),pricing_slot=SLOT,pricing_implementation=impl,
                pricing_impl_code=code(impl),vault=vault,pairs=markets,reads=c.calls,cases=cases)


if __name__=="__main__":
    write(FOLDER/"elfomofi_base.json",capture())
