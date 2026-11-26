"""Base launch curves, actual denomination, pool registration, public bonding contract and current launch status."""
import argparse
import json
from pathlib import Path
from eth_abi import decode
from eth_utils import keccak
from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT

REGISTRY={
    "virtuals-base":("0x158d7ccaa23dc3c8861c3323ed546e3d25e74309","0x8292b43ab73efac11faf357419c38acf448202c5",
                     ["0xf66dea7b3e897cd44a5a231c61b6b4423d613259"],
                     ["0x8c7857a0e5c9bf5f9e4b18e4b761018cb1e2324d","0x4c0ad635f1c9d3c3a6f57593eba1bed7b5ff9720","0xd65ead50f1ba26695f099605b7ca1bc9c4015943"]),
    "virtuals-unicorn-base":("0xd7d3c85b4f2e9bee1998cd2e98820e647792d284","0xbfce3fbe9ce3a19adb8dbb096ea2cb2bb1073f95",
                     ["0xc3538ddd84619e761b4c03caf2f785f79889958d","0xa31bd6a0edbc4da307b8fa92bd6cf39e0fae262c"],
                     ["0x2872b72ebf3383e544058a799e37bc7dfb8fab54","0xa432e195535eef6cdfc621002b0134e0f65cc7ed","0xdbe9722f44b984fba6cc608da9f726c347515fb8"])
}
BASE="0x0b3e328455c4059eeb9e3f84b5543f74e24e7e1b"
SLOT="0x"+format(int.from_bytes(keccak(text="eip1967.proxy.implementation"),"big")-1,"064x")
ENDPOINTS=["https://mainnet.base.org","https://base.gateway.tenderly.co","https://base-rpc.publicnode.com"]

def read(c,method,params):
    r=c.session.post(c.endpoint,json=dict(jsonrpc="2.0",id=1,method=method,params=params),timeout=20)
    r.raise_for_status();j=r.json()
    if "result" not in j:raise ValueError(j.get("error"))
    return j["result"]

def one(c,addr,sig,ret,args=(),vals=()):
    return c.one(addr,sig,args,vals,ret)

def role(c,address):
    code=read(c,"eth_getCode",[address,c.selector]);slot=read(c,"eth_getStorageAt",[address,SLOT,c.selector])
    if code=="0x":raise ValueError("empty role")
    v=dict(address=address,hash="0x"+keccak(hexstr=code).hex(),size=(len(code)-2)//2)
    if int(slot,16):
        impl="0x"+slot[-40:];runtime=read(c,"eth_getCode",[impl,c.selector])
        v.update(implementation=impl,implementation_hash="0x"+keccak(hexstr=runtime).hex())
    return v

def capture(id,ep):
    cfg=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/base.json").read_bytes());cfg["endpoint"]=ep;c=Capture(cfg)
    fac,router,fronts,pools=REGISTRY[id]
    addr=lambda to,sig:one(c,to,sig,["address"])[0]
    num=lambda to,sig:one(c,to,sig,["uint256"])[0]
    if addr(router,"factory()")!=fac or addr(router,"assetToken()")!=BASE:raise ValueError("wrong router")
    roles={"factory":role(c,fac),"router":role(c,router),"asset":role(c,BASE)}
    executor=keccak(text="EXECUTOR_ROLE")
    for i,f in enumerate(fronts):
        if addr(f,"factory()")!=fac or addr(f,"router()")!=router or not one(c,router,"hasRole(bytes32,address)",["bool"],["bytes32","address"],[executor,f])[0]:raise ValueError("foreign entry")
        roles["front"+str(i)]=role(c,f)
    available=num(fac,"allPairsLength()");fees=[num(fac,n+"Tax()") for n in ["buy","sell"]]
    samples=[]
    for pool in pools:
        a,b=addr(pool,"tokenA()"),addr(pool,"tokenB()")
        if b!=BASE or addr(pool,"router()")!=router or one(c,fac,"getPair(address,address)",["address"],["address","address"],[a,b])[0]!=pool:raise ValueError("unregistered pair")
        k=num(pool,"kLast()");res=list(one(c,pool,"getReserves()",["uint256","uint256"]))
        status=[]
        for f in fronts:
            raw,=c.batch([(f,"tokenInfo(address)",["address"],[a])])
            if raw is None:raise ValueError("missing launch status")
            # ABI encodes four addresses then an offset to the dynamic Data tuple, six strings and the two trading flags.
            types=["address"]*4+["uint256"]*7+["bool","bool"]
            h=decode(types,raw[:32*len(types)]);status.append(dict(entry=f,creator=h[0],token=h[1],pair=h[2],agent=h[3],trading=h[11],graduated=h[12]))
        balances=[one(c,t,"balanceOf(address)",["uint256"],["address"],[pool])[0] for t in [a,b]]
        samples.append(dict(pool=pool,token=a,reserves=list(map(str,res)),balances=list(map(str,balances)),k=str(k),status=status))
    return dict(id=id,selector=c.selector,roles=roles,registry_count=available,fees=fees,samples=samples,raw=c.calls)
if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("id",choices=REGISTRY);parser.add_argument("output",type=Path);args=parser.parse_args()
    for ep in ENDPOINTS:
        try:
            data=capture(args.id,ep);args.output.write_text(json.dumps(data,indent=2)+"\n",encoding="utf8",newline="\n");break
        except (ValueError,OSError,KeyError) as exc:print(ep,str(exc)[:200],flush=True)
    else:raise ValueError("no accepted Virtuals snapshot")
