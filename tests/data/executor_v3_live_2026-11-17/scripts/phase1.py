import sys; sys.path.insert(0,"/tmp/claude-1000/ex/run")
from common import *
import os
n=Node(8545)
H=h2i(n.rpc("eth_blockNumber",[])["result"])
hb=n.rpc("eth_getBlockByNumber",[hex(H),False])["result"]
print("H",H,"time",h2i(hb["timestamp"]),flush=True)
U="0x"+ad(USDT)
fam={
 "v4":(V4PM,topic('Initialize(bytes32,address,address,uint24,int24,address,uint160,int24)')),
 "cl":(CLPM,topic('Initialize(bytes32,address,address,address,uint24,bytes32,uint160,int24)')),
 "bin":(BINPM,topic('Initialize(bytes32,address,address,address,uint24,bytes32,uint24)')),
}
start=H-595000; step=5000
out={}
for f,(addr,t0) in fam.items():
    evs=[]
    for lo in range(start,H+1,step):
        hi=min(lo+step-1,H)
        for pos in (2,3):
            tp=[t0,None,None,None]; tp[pos-1]=U
            tp=tp[:pos+1] if False else [t0]+[None]*(pos-1)+[U]
            r=n.rpc("eth_getLogs",[{"fromBlock":hex(lo),"toBlock":hex(hi),"address":addr,"topics":tp}])
            if "error" in r: print(f,lo,"ERR",r["error"],flush=True); continue
            evs+=r["result"]
        time.sleep(0.02)
    print(f,"events with USDT",len(evs),flush=True)
    out[f]=evs
json.dump({"H":H,"time":h2i(hb["timestamp"]),"events":out},open("phase1.json","w"))
json.dump(n.log,open("phase1_rpc.json","w"))
print("done",flush=True)
