"""Проверка создания модуля Aqua, сопоставления программы V5 и реальной выплаты."""
from hashlib import sha256
from eth_abi import encode, decode
from eth_utils import keccak

from aqua_evidence import ORDER, REGISTRY, ROUTER
from record_base_slipstream_executor import EXEC, OWNER, RUNTIME, STEP, rpc, simulate_block
from record_bitswap_ethereum import balance, tx

TERMS = "(address,address,uint256,uint256,uint256,bool)"
SETTLED = keccak(text="Settled(bytes32,address,address,address,address,uint256,uint256,bool)")


class Guard:
    def __init__(self, url, parent, artifact, wrapped, order, mode):
        self.mode = mode
        self.order = order
        self.digest = keccak(encode([ORDER], [order]))
        if int(rpc(url, "eth_getTransactionCount", [OWNER, parent]), 16):
            raise ValueError("isolated owner already used")
        self.module = "0x" + keccak(b"\xd6\x94" + bytes.fromhex(OWNER[2:]) + b"\x80")[-20:].hex()
        for target in (self.module, EXEC, OWNER):
            if rpc(url, "eth_getCode", [target, parent]) != "0x":
                raise ValueError("isolated address occupied")
        runtime = bytes.fromhex(RUNTIME.read_text().strip().removeprefix("0x"))
        params = encode(["address","address","bytes32"], [ROUTER,REGISTRY,keccak(runtime)])
        creation = bytes.fromhex(artifact["creation"]) + params
        slots = {f"0x{i:064x}":f"0x{int(a,16):064x}" for i,a in enumerate((OWNER,wrapped))}
        self.env = {OWNER:{"balance":hex(10**20)}, EXEC:{"code":"0x"+runtime.hex(),"stateDiff":slots}}
        cache = tx(self.module, "cache("+ORDER+")", [ORDER], [order])
        create = {"from":OWNER,"data":"0x"+creation.hex(),"gas":hex(6_000_000)}
        created, registered, repeated = simulate_block(url, parent, self.env, [create,cache,cache])
        if [v["status"] for v in (created,registered,repeated)] != ["0x1"]*3:
            raise ValueError(("actual creation or program registration",created.get("error"),registered.get("error")))
        code = bytes.fromhex(created["returnData"][2:])
        normalized = bytearray(code)
        for refs in artifact["immutableReferences"].values():
            for ref in refs:
                i,n = ref["start"],ref["length"]
                normalized[i:i+n] = bytes(n)
        if normalized.hex()!=artifact["runtime"] or repeated["logs"] or len(registered["logs"]) != 1:
            raise ValueError("compiled program or duplicate registration")
        if registered["returnData"]!="0x"+self.digest.hex() or repeated["returnData"]!=registered["returnData"]:
            raise ValueError("order binding on registration")
        self.env[self.module]={"code":"0x"+code.hex()}
        self.prefix=[cache] if mode=="v5" else []
        self.endpoint=EXEC if mode=="v5" else self.module
        self.manifest=dict(module=self.module,engine=ROUTER,registry=REGISTRY,code_hash="0x"+keccak(code).hex(),
                           v5_hash="0x"+keccak(runtime).hex(),creation="0x"+creation.hex(),creation_gas=int(created["gasUsed"],16),
                           cache_gas=int(registered["gasUsed"],16),repeat_cache_gas=int(repeated["gasUsed"],16),
                           runtime_sha256=sha256(code).hexdigest(),cache_data=cache["data"])

    def wire(self, src, dst, amount, minimum, deadline, flags, unknown=False):
        first=flags==0x61
        if self.mode=="wallet":
            return tx(self.module,"execute("+ORDER+","+TERMS+")",[ORDER,TERMS],
                      [self.order,(src,dst,amount,minimum,deadline,first)])
        h=bytes(32) if unknown else keccak(encode(["bytes32","bool"],[self.digest,first]))
        step=(17,self.module,src,dst,10000,h)
        return tx(EXEC,"execute(address,uint256,address,uint256,uint256,"+STEP+"[])",
                  ["address","uint256","address","uint256","uint256",STEP+"[]"],[src,amount,dst,minimum,deadline,[step]])

    def quote(self,src,dst,amount,minimum,deadline,flags):
        return tx(self.module,"preview("+ORDER+","+TERMS+",address)",[ORDER,TERMS,"address"],
                  [self.order,(src,dst,amount,minimum,deadline,flags==0x61),OWNER])

    def marks(self,src,dst):
        return [balance(t,a) for a in (self.module,EXEC) for t in (src,dst)]+[
            tx(src,"allowance(address,address)",["address","address"],[a,b]) for a,b in
            ((self.module,ROUTER),(EXEC,self.module))]

    def evidence(self,logs,src,dst,paid,got,flags):
        matches=[l for l in logs if l["address"].lower()==self.module and l["topics"][0].lower()=="0x"+SETTLED.hex()]
        if len(matches)!=1: raise ValueError("ambiguous protected settlement")
        item=matches[0]
        if item["topics"][1].lower()!="0x"+self.digest.hex() or int(item["topics"][2],16)!=int(EXEC if self.mode=="v5" else OWNER,16) or int(item["topics"][3],16)!=int(OWNER,16):
            raise ValueError("unexpected program, payer or destination")
        if decode(["address","address","uint256","uint256","bool"],bytes.fromhex(item["data"][2:]))!=(src,dst,paid,got,flags==0x61):
            raise ValueError("settlement disagrees with physical balances")
