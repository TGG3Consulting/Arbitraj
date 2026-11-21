import json,time,http.client,threading
# keccak-256 (чистый python)
RC=[0x0000000000000001,0x0000000000008082,0x800000000000808A,0x8000000080008000,0x000000000000808B,0x0000000080000001,0x8000000080008081,0x8000000000008009,0x000000000000008A,0x0000000000000088,0x0000000080008009,0x000000008000000A,0x000000008000808B,0x800000000000008B,0x8000000000008089,0x8000000000008003,0x8000000000008002,0x8000000000000080,0x000000000000800A,0x800000008000000A,0x8000000080008081,0x8000000000008080,0x0000000080000001,0x8000000080008008]
ROT=[[0,36,3,41,18],[1,44,10,45,2],[62,6,43,15,61],[28,55,25,21,56],[27,20,39,8,14]]
M=(1<<64)-1
def rol(x,n): n%=64; return ((x<<n)|(x>>(64-n)))&M if n else x
def f(A):
    for rc in RC:
        C=[A[x][0]^A[x][1]^A[x][2]^A[x][3]^A[x][4] for x in range(5)]
        D=[C[(x-1)%5]^rol(C[(x+1)%5],1) for x in range(5)]
        A=[[A[x][y]^D[x] for y in range(5)] for x in range(5)]
        B=[[0]*5 for _ in range(5)]
        for x in range(5):
            for y in range(5): B[y][(2*x+3*y)%5]=rol(A[x][y],ROT[x][y])
        A=[[B[x][y]^((~B[(x+1)%5][y])&B[(x+2)%5][y]) for y in range(5)] for x in range(5)]
        A[0][0]^=rc
    return A
def keccak(data:bytes)->bytes:
    rate=136; p=bytearray(data); p.append(0x01)
    while len(p)%rate: p.append(0)
    p[-1]|=0x80
    A=[[0]*5 for _ in range(5)]
    for i in range(0,len(p),rate):
        blk=p[i:i+rate]
        for j in range(rate//8):
            A[j%5][j//5]^=int.from_bytes(blk[8*j:8*j+8],"little")
        A=f(A)
    out=b""
    for j in range(4): out+=A[j%5][j//5].to_bytes(8,"little")
    return out[:32]
def sel(sig): return keccak(sig.encode())[:4].hex()
def topic(sig): return "0x"+keccak(sig.encode()).hex()
def w(x): return (x&(2**256-1)).to_bytes(32,"big").hex()
def ad(a): return a[2:].lower().rjust(64,"0")
def s24(x): x&=0xffffff; return x-(1<<24) if x>>23 else x
def s256(x): return x-(1<<256) if x>>255 else x
class Node:
    def __init__(s,port=8545):
        s.port=port; s.log=[]; s.lock=threading.Lock()
    def rpc(s,m,p):
        c=http.client.HTTPConnection("127.0.0.1",s.port,timeout=60)
        req={"jsonrpc":"2.0","id":1,"method":m,"params":p}
        t=time.time()
        c.request("POST","/",json.dumps(req),{"content-type":"application/json"})
        r=json.loads(c.getresponse().read())
        with s.lock: s.log.append({"t":round(t,3),"method":m,"params":p,**({"result":r["result"]} if "result" in r else {"error":r.get("error")})})
        return r
    def call(s,to,data,blk,frm=None):
        o={"to":to,"data":data}
        if frm: o["from"]=frm
        return s.rpc("eth_call",[o,blk])
