"""Read-only evidence of Mint Club ERC20 deployments, token registration and actual creation events."""
import argparse
import gzip
import json
from pathlib import Path
from eth_abi import decode, encode
from eth_utils import keccak
from record_mint_club import Archive, BOND, TOKEN, unpack
from record_base_slipstream_executor import calldata


def capture(network):
    d=Path(__file__).resolve().parents[1]/"tests/data/first_run_dex_2026-11-26"
    wallet=json.loads(gzip.decompress((d/f"mint_club_{network}_wallets.json.gz").read_bytes()))
    arc=Archive(network)
    if network=="base":
        for url in ["https://base-rpc.publicnode.com","https://base.gateway.tenderly.co"]:
            def read(method,params):return arc.session.post(url,json={"jsonrpc":"2.0","id":1,"method":method,"params":params},timeout=10).json().get("result")
            try:
                h=read("eth_getBlockByHash",[arc.tag["blockHash"],False])
                if read("eth_chainId",[])==hex(arc.chain) and h and h["stateRoot"]==arc.header["stateRoot"] and int(h["number"],16)==arc.height:
                    arc.endpoints=[url]+arc.endpoints
            except (ValueError,OSError):continue
    if arc.tag!=wallet["selector"] or arc.header["stateRoot"]!=wallet["state_root"]:
        raise ValueError("wrong canonical snapshot")
    contracts={}
    for addr in (BOND,"0x"+TOKEN):
        code=arc.code(addr)
        contracts[addr]={"bytes":len(code),"keccak256":"0x"+keccak(code).hex()}
        if not code:raise ValueError("deployment missing")
    topic="0x"+keccak(text="TokenCreated(address,string,string,address)").hex()
    clone=bytes.fromhex("363d3d373d3d3d363d73"+TOKEN+"5af43d82803e903d91602b57fd5bf3")
    rows=[]; heads={arc.height:arc.header}
    def header(n):
        if n not in heads:heads[n]=arc.rpc("eth_getBlockByNumber",[hex(n),False]);
        return heads[n]
    for m in wallet["markets"]:
        token=m["token"]; reserve=m["reserve"]
        terms_raw=arc.rpc("eth_call",[{"to":BOND,"data":calldata("tokenBond(address)",["address"],[token])},arc.tag])
        terms=decode(["address","uint16","uint16","uint40","address","uint256"],unpack(terms_raw))
        created=terms[3]
        if arc.code(token)!=clone or terms[4]!=reserve:raise ValueError("registration changed")
        lo,hi=1,arc.height
        n=max(lo,hi-(int(arc.header["timestamp"],16)-created)//(2 if network=="base" else 12))
        while lo<hi:
            current=int(header(n)["timestamp"],16)
            if current==created:break
            if current>created:hi=n-1
            else:lo=n+1
            n=(lo+hi)//2
        h=header(n)
        if int(h["timestamp"],16)!=created:raise ValueError("creation timestamp not found")
        topics=[topic,"0x"+encode(["address"],[token]).hex(),"0x"+encode(["address"],[reserve]).hex()]
        logs=arc.rpc("eth_getLogs",[{"blockHash":h["hash"],"address":BOND,"topics":topics}]);
        if len(logs)!=1:raise ValueError("missing or duplicate verified TokenCreated")
        log=logs[0]; name,symbol=decode(["string","string"],unpack(log["data"]))
        if log["address"].lower()!=BOND or log["topics"]!=topics or log["blockHash"]!=h["hash"] or log.get("removed"):raise ValueError("unbound event")
        token_at_index,=arc.view(BOND,"tokens(uint256)",["uint256"],[m["index"]],["address"])
        if token_at_index!=token:raise ValueError("registry index mismatch")
        physical,=arc.view(reserve,"balanceOf(address)",["address"],[BOND]);
        quotes=[]
        for trial in m["trials"]:
            amount=trial["size"]
            q={"size":amount}
            for label,name in [("mint","getReserveForToken(address,uint256)"),("burn","getRefundForTokens(address,uint256)")]:
                payload=calldata(name,["address","uint256"],[token,int(amount)])
                if label=="burn" and int(amount)>int(m["supply"]):
                    reply=arc.session.post(arc.endpoints[0],json={"jsonrpc":"2.0","id":1,"method":"eth_call","params":[{"to":BOND,"data":payload},arc.tag]},timeout=20).json()
                    if reply.get("error",{}).get("data")!="0x"+keccak(text="MCV2_Bond__ExceedTotalSupply()")[:4].hex():raise ValueError("unexpected burn revert")
                    q["burn_error"]=reply["error"]["data"]
                else:q[label]=arc.rpc("eth_call",[{"to":BOND,"data":payload},arc.tag])
            if decode(["uint256","uint256"],unpack(q["mint"]))[0]!=int(trial["mint_cost"]):raise ValueError("mint getter disagrees with wallet")
            quotes.append(q)
        rows.append({"token":token,"terms_raw":terms_raw,"quotes":quotes,"index":m["index"],"reserve":reserve,"bonded":str(terms[-1]),"available":str(physical),"clone_code":"0x"+clone.hex(),"name":name,"symbol":symbol,"creation_time":created,"creation":log})
    # The shared registry has both ERC20 and ERC1155 clones. A positive tokenBond entry by itself is insufficient.
    nft=None
    for index,(token,) in enumerate(arc.many(BOND,"tokens(uint256)",["uint256"],list(range(min(24,wallet["total"]))),["address"])):
        code=arc.code(token)
        if len(code)!=45 or code==clone:continue
        supported,=arc.view(token,"supportsInterface(bytes4)",["bytes4"],[bytes.fromhex("d9b67a26")],["bool"])
        if not supported:continue
        terms=arc.rpc("eth_call",[{"to":BOND,"data":calldata("tokenBond(address)",["address"],[token])},arc.tag]);
        nft={"token":token,"index":index,"clone_code":"0x"+code.hex(),"terms_raw":terms,"erc1155_interface":"0xd9b67a26"};break
    if nft is None:raise ValueError("ERC1155 registry negative control missing")
    events=["0x"+keccak(text=name+"(address,address,address,uint256,address,uint256)").hex() for name in ["Mint","Burn"]]
    # A short, explicit observation interval. Pin individual delivered receipts in the read-only evidence.
    begin=arc.height-(3600 if network=="base" else 600)+1
    logs=[]
    for lo in range(begin,arc.height+1,450):
        hi=min(lo+449,arc.height)
        logs.extend(arc.rpc("eth_getLogs",[{"fromBlock":hex(lo),"toBlock":hex(hi),"address":BOND,"topics":[events]}]))
    samples=[]
    for topic in events:
        sample=next((l for l in logs if l["topics"][0]==topic),None)
        if sample is not None:
            number=int(sample["blockNumber"],16)
            if not begin<=number<=arc.height or sample["blockHash"]!=header(number)["hash"] or sample.get("removed"):raise ValueError("stale execution event")
            samples.append(sample)
    window={"from":begin,"to":arc.height,"counts":[sum(l["topics"][0]==t for l in logs) for t in events],"samples":samples}
    return {"chain_id":arc.chain,"block":arc.height,"selector":arc.tag,"state_root":arc.header["stateRoot"],"contracts":contracts,"markets":rows,"erc1155":nft,"window":window}

if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("network",choices=["base","ethereum"]);parser.add_argument("output",type=Path);args=parser.parse_args()
    args.output.write_text(json.dumps(capture(args.network),ensure_ascii=False,indent=2)+"\n",encoding="utf8",newline="\n")
