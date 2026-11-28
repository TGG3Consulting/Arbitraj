"""Before and after a signature-verified, historically mined oracle update, on its canonical parent."""
from eth_account import Account
from eth_account.typed_transactions import TypedTransaction
from eth_utils import keccak
from hexbytes import HexBytes

FRAMES = {
    26492541: ("0x2736752261a05d8d82aba8f47c2b5b8311412805cb4630968b6156e1e030b6a5", "0xf639cf213b63f7e77d699ff686d591c0ba55fc63", "0x008dcc8e"),
    26494725: ("0xb409a77781afa1e5cdbe70d62996fa423e7d581c6078190d5c138747bdf58db7", "0xda7afeed021eafc1c1af9c362de477dad0396b81", "0xa9114b0f"),
    26492615: ("0xefaae2ddfbd56997bed704f0424d077276b353666ad89817f8929cd0a34f476c", "0xda7afeed021eafc1c1af9c362de477dad0396b81", "0xa9114b0f"),
    26492655: ("0xb0bcb022954dd6546e06a3a4dca6d2b1a286efce998197e37866ba3a26c0efb3", "0xda7afeed021eafc1c1af9c362de477dad0396b81", "0xa9114b0f"),
}

def frame(c, rpc, height, expected, source=None):
    block = rpc(c, "eth_getBlockByNumber", [hex(height), True])["result"]
    assert block["hash"] == expected
    witness, oracle, selector = source if source is not None else FRAMES[height]
    txs = block["transactions"]
    stop = [t["hash"] for t in txs].index(witness)
    parent = {"blockHash": block["parentHash"], "requireCanonical": True}
    updates, digests, nonces = [], [], {}
    for t in txs[:stop]:
        if t["to"] != oracle or not t["input"].startswith(selector):
            continue
        assert int(t["type"],16) == 2 and int(t["chainId"],16) == 1 and t["blockHash"] == expected
        fields = {k:int(t[k],16) for k in ("chainId","type","nonce","value","gas","maxFeePerGas","maxPriorityFeePerGas","r","s","v")}
        fields.update(data=HexBytes(t["input"]),to=HexBytes(t["to"]),accessList=t["accessList"])
        signed = TypedTransaction.from_dict(fields).encode()
        assert "0x"+keccak(signed).hex() == t["hash"] and Account.recover_transaction(signed).lower() == t["from"]
        if t["from"] not in nonces:
            nonces[t["from"]] = int(rpc(c,"eth_getTransactionCount",[t["from"],parent])["result"],16)
        assert fields["nonce"] == nonces[t["from"]]
        nonces[t["from"]] += 1
        rec = rpc(c,"eth_getTransactionReceipt",[t["hash"]])["result"]
        assert rec["blockHash"] == expected and rec["status"] == "0x1"
        call = {k:t[k] for k in ("from","to","value","nonce","gas","maxFeePerGas","maxPriorityFeePerGas","accessList")}
        updates.append(dict(call,data=t["input"]))
        digests.append(t["hash"])
    assert updates
    fields = {"time":block["timestamp"],"feeRecipient":block["miner"],"gasLimit":block["gasLimit"],
              "baseFeePerGas":block["baseFeePerGas"],"prevRandao":block["mixHash"]}
    return parent, fields, updates, {"witness":witness,"index":stop,"updates":digests,"canonicalHash":expected}
