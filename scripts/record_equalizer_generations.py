"""Bind prior Base Equalizer PairFactory implementations and pairCodeHash() to their deployment blocks."""
import re

from eth_utils import keccak

from record_balancer_markets import Capture
from record_woo_base import rpc

# First pools of the first two distinct CREATE2 generations in the factory's ordered registry.
CREATION = ((0,4334766),(32,4938760))
METHODS = ("getAmountOut","_getAmountOut","_k","_get_y","_d","_f","swap","_update0","_update1","_update")


def swap_source(files):
    text=next(v["content"] for v in files.values() if "function _get_y(" in v["content"] and "getAmountOut(" in v["content"])
    text=re.sub(r"/\*.*?\*/|//[^\n]*","",text,flags=re.S)
    text=text[re.search(r"\bcontract\s+Pair\b",text).start():]
    result={}
    for func in METHODS:
        a=text.index("function "+func+"(");b=text.index("{",a);c=b+1;depth=1
        while depth:
            depth += (text[c]=="{")-(text[c]=="}")
            c+=1
        result[func]="0x"+keccak(text=re.sub(r"\s+"," ",text[a:c])).hex()
    return result


def generations(endpoint, factory, pools, current, verified_source, slot):
    methods=swap_source(current)
    out=[]
    node={"endpoint":endpoint}
    for index,height in CREATION:
        parent=rpc(node,"eth_getBlockByNumber",[hex(height),False])
        tag={"blockHash":parent["hash"],"requireCanonical":True}
        pool=pools[index]
        if rpc(node,"eth_getCode",[pool,hex(height-1)])!="0x" or rpc(node,"eth_getCode",[pool,tag])=="0x":
            raise ValueError("historical pair birth differs")
        raw=rpc(node,"eth_getStorageAt",[factory,slot,tag])
        implementation="0x"+raw[-40:]
        then=Capture({"endpoint":endpoint,"selector":tag})
        src,files=verified_source(then,implementation)
        if swap_source(files)!=methods:
            raise ValueError("historical quote, settlement or fee functions differ")
        init,=then.one(factory,"pairCodeHash()",returns=["bytes32"])
        out.append(dict(first=index,pool=pool,block=height,selector=tag,state_root=parent["stateRoot"],
                        implementation=src,init="0x"+init.hex(),swap_source=methods))
    return out
