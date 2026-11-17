import sys,json,time
sys.path.insert(0,"/tmp/claude-1000/ex/run")
from eth import *
USDT="0x55d398326f99059ff775485246999027b3197955"
WBNB="0xbb4cdb9cbd36b01bd1cbaebf2de08d9173bc095c"
W="0x1c8f435a7a86a47490e9c9e15cd1af573985d94d"
V4PM="0x28e2ea090877bf75740558f6bfb36a5ffee9e9df"; V4Q="0x95c086d7fe28add842ee220fef225a777474d781"
VAULT="0x238a358808379702088667322f80ac48bad5e6c4"
CLPM="0xa0ffb9c1ce1fe56963b0321b32e7a0302114058b"; CLQ="0xd0737c9762912dd34c3271197e362aa736df0926"
BINPM="0xc697d2898e0d09264376196696c51d7abbbaa4a9"; BINQ="0xc631f4b0fc2dd68ad45f74b2942628db117dd359"
V2F="0xca143ce32fe78f1f7019d7d551a6402fc5350c73"
MAXU=2**256-1
def h2i(x): return int(x,16)
def word(x,i): return int(x[2+64*i:2+64*(i+1)],16)
def sw(x,i):
    v=word(x,i); return v-(1<<256) if v>>255 else v
def adr(x,i): return "0x"+x[2+64*i+24:2+64*(i+1)]
def enc_static(t,v):
    if t=="address": return ad(v)
    if t=="bool": return w(1 if v else 0)
    if t.startswith("uint") or t.startswith("int"): return w(v)
    if t=="bytes32": return v[2:].rjust(64,"0")
    raise Exception(t)
def enc_bytes(b):
    b=bytes.fromhex(b[2:]) if isinstance(b,str) else b
    return w(len(b))+(b.hex()+"00"*((-len(b))%32))
def enc_tuple(types,vals):
    # types: list of "type" or ("bytes",) dynamic handled
    head="";tail="";hs=32*len(types)
    for t,v in zip(types,vals):
        if t=="bytes":
            head+=w(hs+len(tail)//2); tail+=enc_bytes(v)
        elif t=="step[]":
            head+=w(hs+len(tail)//2); tail+=enc_steps(v)
        elif t=="tuple_dyn": raise Exception
        else: head+=enc_static(t,v)
    return head+tail
def enc_steps(steps):
    n=len(steps); head=w(n); bodies=[]
    for s in steps: bodies.append(enc_tuple(["uint8","address","address","address","uint16","bytes"],[s["kind"],s["pool"],s["tokenIn"],s["tokenOut"],s["shareBps"],s["data"]]))
    offs=[];cur=32*n
    for b in bodies: offs.append(cur); cur+=len(b)//2
    return head+"".join(w(o) for o in offs)+"".join(bodies)
EXEC_SIG="execute(address,uint256,address,uint256,uint256,(uint8,address,address,address,uint16,bytes)[])"
def enc_execute(tin,amt,tout,minout,dl,steps):
    return "0x"+sel(EXEC_SIG)+enc_tuple(["address","uint256","address","uint256","uint256","step[]"],[tin,amt,tout,minout,dl,steps])
def key_v4(k): return "0x"+enc_tuple(["address"]*2+["uint24","int24","address"],[k["c0"],k["c1"],k["fee"],k["ts"]&0xffffff if k["ts"]<0 else k["ts"],k["hooks"]]) if False else "0x"+ad(k["c0"])+ad(k["c1"])+w(k["fee"])+w(k["ts"])+ad(k["hooks"])
def key_inf(k): return "0x"+ad(k["c0"])+ad(k["c1"])+ad(k["hooks"])+ad(k["pm"])+w(k["fee"])+k["params"][2:].rjust(64,"0")
def pid_of(keybytes): return "0x"+keccak(bytes.fromhex(keybytes[2:])).hex()
