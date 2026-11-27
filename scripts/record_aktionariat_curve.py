"""Extract measured Brokerbot curves and original source rounding for the tested Ethereum issuers."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path

import requests
from eth_utils import keccak
from record_aktionariat_ethereum import market_views
from fork_fee_rpc import Reader
from record_llama_base_cl import CanonicalCalls
from record_base_slipstream_executor import ROOT

DATA = ROOT / "tests/data/first_run_dex_2026-11-26"

def capture():
    registry = json.loads(gzip.decompress((DATA / "aktionariat_ethereum_directory.json.gz").read_bytes()))
    wallet = json.loads(gzip.decompress((DATA / "aktionariat_ethereum_native.json.gz").read_bytes()))
    head = json.loads((DATA / "ethereum.json").read_bytes())
    cap = CanonicalCalls(Reader("ethereum",head))
    assert registry["selector"] == wallet["selector"] == cap.reader.tag
    accepted = sorted({x["market"] for x in wallet["controls"] if x.get("buy_received")})
    rows = []
    for address in accepted:
        m=registry["markets"][address]
        api="https://eth.blockscout.com/api/v2/smart-contracts/"+address
        response=requests.get(api,timeout=15)
        response.raise_for_status()
        source=response.json()
        blob=source["source_code"]
        runtime=keccak(hexstr=source["deployed_bytecode"])
        assert "0x"+runtime.hex()==registry["roles"][address]["runtime"] and source["is_verified"]
        fragment=" ".join(blob.split())
        variants={"average":"return ((lowest + highest) / 2) * shares;", "total":"return (lowest + highest) * shares / 2;"}
        rounding,=[key for key,s in variants.items() if s in fragment]
        licensed=any(x.get("name")=="getLicenseFee" for x in source["abi"])
        price,step,drift,owner,supply=market_views(cap,[
            (address,"getPrice()",[],[],"uint256"),(address,"increment()",[],[],"uint256"),(address,"hasDrift()",[],[],"bool"),
            (address,"owner()",[],[],"address"),(m["share"],"totalSupply()",[],[],"uint256")])
        assert None not in (price,step,drift,owner,supply)
        if drift:
            start,=cap.one(address,"driftStart()",returns=["uint256"])
            base,=cap.one(address,"getPriceAtTime(uint256)",["uint256"],[start],["uint256"])
        else:
            base=price
        fee=0
        if licensed:
            fee,=cap.one(address,"getLicenseFee(uint256)",["uint256"],[10000],["uint256"])
            assert "return totPrice * licenseFeeBps / 10000;" in fragment
        origin="registry"
        if m["registered_pair"] is None:
            share_owner,=cap.one(m["share"],"owner()",returns=["address"])
            assert share_owner==owner
            origin=owner
        setting=" ".join([address,m["cash"],m["share"],m["hub"],registry["roles"][address]["runtime"],
                          registry["roles"][m["hub"]]["runtime"],origin,rounding,str(fee)])
        rows.append(dict(broker=address,setting=setting,source=api,source_sha256=hashlib.sha256(blob.encode()).hexdigest(),
                         price=str(price),base_price=str(base),increment=str(step),supply=str(supply),drift=drift))
    return dict(height=registry["height"],selector=registry["selector"],rows=rows)

if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    parser.parse_args().output.write_text(json.dumps(capture(),indent=1)+"\n",encoding="utf8")
