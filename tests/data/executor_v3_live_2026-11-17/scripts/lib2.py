import sys; sys.path.insert(0,"/tmp/claude-1000/ex/run")
from common import *
class Ctx: pass
def getn(): 
    n=Node(8545); return n
def call(n,to,data,H,frm=None):
    r=n.call(to,data,hex(H),frm)
    return r["result"] if "result" in r else None
def call_err(n,to,data,H,frm=None):
    r=n.call(to,data,hex(H),frm)
    return (r["result"],None) if "result" in r else (None,json.dumps(r.get("error"))[:300])
def bal(n,tok,a,H):
    r=call(n,tok,"0x70a08231"+ad(a),H); return word(r,0) if r and len(r)>=66 else None
def alw(n,tok,o,s,H):
    r=call(n,tok,"0xdd62ed3e"+ad(o)+ad(s),H); return word(r,0) if r and len(r)>=66 else None
def stor(n,a,slot,H):
    r=n.rpc("eth_getStorageAt",[a,slot,hex(H)]); return h2i(r["result"]) if "result" in r else None
def mapslot(a,i): return "0x"+keccak(bytes.fromhex(ad(a)+w(i))).hex()
def allowslot(owner,spender,i):
    inner=keccak(bytes.fromhex(ad(owner)+w(i)))
    return "0x"+keccak(bytes.fromhex(ad(spender))+inner if False else bytes.fromhex(ad(spender))+inner).hex()
def hexslot(i): return "0x"+w(i)
def padv(v): return "0x"+w(v)
def sim(n,H,overrides,calls,blockoverrides=None):
    st={"stateOverrides":overrides,"calls":calls}
    if blockoverrides: st["blockOverrides"]=blockoverrides
    r=n.rpc("eth_simulateV1",[{"blockStateCalls":[st],"validation":False},hex(H)])
    return r
def decode_revert(rd,err=None):
    if rd and rd.startswith("0x08c379a0") and len(rd)>=138:
        try:
            ln=int(rd[2+8+64:2+8+128],16); return "Error("+bytes.fromhex(rd[2+8+128:2+8+128+2*ln]).decode("utf8","replace")+")"
        except Exception: pass
    if rd and rd.startswith("0x4e487b71"): return "Panic("+str(int(rd[10:74] or "0",16))+")"
    if rd and rd!="0x": return "sel "+rd[:10]+" data "+rd[:200]
    return (err or {}).get("message","") if isinstance(err,dict) else ""
