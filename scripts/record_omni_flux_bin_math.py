"""Снимок бинов ДО каждой реальной ноги: вторая нога включает исполнение первой в том же состоянии."""
from eth_abi import decode
from eth_utils import keccak

TOPIC="0x"+keccak(text="Swap(bytes32,address,int128,int128,uint24,uint24,uint16)").hex()

def before(k,pid,prefix,mode,owner,forward,receipt,tx,simulate):
    manager=k[3];encoded=bytes.fromhex(pid[2:])
    def call(sig,types,args):
        return tx(manager,sig,types,args)
    def views(queries):
        got=simulate(owner,prefix+queries,mode)[-len(queries):]
        if any(r["status"]!="0x1" for r in got):raise ValueError("состояние Bin не прочитано")
        return [bytes.fromhex(r["returnData"][2:]) for r in got]
    slot,=views([call("getSlot0(bytes32)",["bytes32"],[encoded])])
    active,protocol,lp=decode(["uint24"]*3,slot)
    logs=[l for l in receipt["logs"] if l["address"].lower()==manager and l["topics"][:2]==[TOPIC,pid]]
    if len(logs)!=1:raise ValueError("не найден уникальный Swap менеджера Bin")
    delta0,delta1,last,fee,pfee=decode(["int128","int128","uint24","uint24","uint16"],bytes.fromhex(logs[0]["data"][2:]))
    if forward!=(delta0<0) or (forward and last>active) or (not forward and last<active):
        raise ValueError("не совпало направление фактического обмена")
    lo,hi=sorted((active,last));ids=[active]
    if hi-lo<=96:ids=list(range(lo,hi+1))
    else:
        while ids[-1]!=last:
            nxt,=views([call("getNextNonEmptyBin(bytes32,bool,uint24)",["bytes32","bool","uint24"],[encoded,forward,ids[-1]])])
            n=decode(["uint24"],nxt)[0]
            if not lo<=n<=hi or (n>=ids[-1] if forward else n<=ids[-1]):raise ValueError("нарушена последовательность непустых бинов")
            ids.append(n)
    amounts=[]
    for start in range(0,len(ids),32):
        chunk=ids[start:start+32]
        amt=views([call("getBin(bytes32,uint24)",["bytes32","uint24"],[encoded,n]) for n in chunk])
        for n,v in zip(chunk,amt):
            x,y,_,_=decode(["uint128","uint128","uint256","uint256"],v)
            amounts.append([n,str(x),str(y)])
    return dict(active=active,last=last,protocol=protocol,lp=lp,event_fee=fee,event_protocol=pfee,
                forward=forward,delta0=str(delta0),delta1=str(delta1),bins=amounts)
