"""Публичные этапы getQuote Tokenlon для нового адреса. Храним только открытые поля ответа."""
import argparse
import json
import secrets
import time
import uuid
from pathlib import Path
from urllib.parse import urlencode

import requests
from eth_account import Account
from eth_account.messages import encode_defunct
from websocket import create_connection, WebSocketException

PUBLISHER = "publisher.tokenlon.im"


def subscription(socket, number, topic, relayer):
    mark = f"q{number}"
    headers = [f"id:{mark}", f"destination:{topic}", "X-TOKENLON-VERSION:5.2.0",
               "X-DEVICE-TOKEN:tokenlon-v5-sdk", f"X-SEND-BY-RELAYER:{str(relayer).lower()}", "X-STRATEGY:PMMV5"]
    socket.send(json.dumps(["SUBSCRIBE\n" + "\n".join(headers) + "\n\n\x00"]))
    until = time.monotonic()+12
    while time.monotonic() < until:
        socket.settimeout(max(0.1, until-time.monotonic()))
        packet = socket.recv()
        if not packet.startswith("a"):
            continue
        for frame in json.loads(packet[1:]):
            if not frame.startswith("MESSAGE\n") or f"subscription:{mark}\n" not in frame:
                continue
            body = frame.split("\n\n",1)[1].rstrip("\x00")
            if body == "None":
                continue
            socket.send(json.dumps([f"UNSUBSCRIBE\nid:{mark}\n\n\x00"]))
            return json.loads(body)
    raise TimeoutError("publication")


def sample(pair, side, size, relayer):
    account = Account.create()
    timestamp = round(time.time())
    proof = account.sign_message(encode_defunct(text=str(timestamp))).signature
    row = dict(pair=pair,side=side,size_base=size,relayer=relayer,utc_unix=timestamp,
               ephemeral=account.address,steps=[])
    socket = None
    stage = "authentication"
    try:
        auth = requests.post(f"https://{PUBLISHER}/rpc",json={"jsonrpc":"2.0","id":1,
            "method":"auth.getSdkJwtToken","params":{"timestamp":timestamp,"signature":"0x"+proof.hex()}},timeout=10)
        auth.raise_for_status()
        jwt = auth.json().get("result")
        if not isinstance(jwt,str) or not jwt:
            raise ValueError("authentication")
        stage = "connection"
        query = urlencode({"Authorization":"Token "+jwt})
        url = f"wss://{PUBLISHER}/exchange/{secrets.randbelow(1000):03d}/{uuid.uuid4().hex[:12]}/websocket?{query}"
        socket = create_connection(url, timeout=12)
        if socket.recv() != "o":
            raise ValueError("SockJS open")
        socket.send(json.dumps(["CONNECT\naccept-version:1.1,1.0\nheart-beat:0,0\n\n\x00"]))
        if "CONNECTED" not in socket.recv():
            raise ValueError("STOMP connect")
        for number, stage in enumerate(("order","lastOrder")):
            message = subscription(socket, number, f"/user/{stage}/{pair}/{side}/{size}/{account.address}",relayer)
            order = message.get("order") or {}
            signature = bytes.fromhex(str(order.get("makerWalletSignature") or "").removeprefix("0x"))
            allowed = ("protocol","makerAddress","takerAddress","makerAssetAddress","takerAssetAddress",
                       "makerAssetAmount","takerAssetAmount","feeFactor","expirationTimeSeconds")
            row["steps"].append({"stage":stage,"ok":message.get("exchangeable"),
                "reason":message.get("reason"),"maker_signature_bytes":len(signature),
                "bound_to_wallet":str(order.get("feeRecipientAddress","")).lower()==account.address.lower(),
                "order":{key:order[key] for key in allowed if key in order}})
            if not message.get("exchangeable"):
                break
    except (requests.RequestException, WebSocketException, OSError, ValueError, KeyError) as error:
        # Exception text from a websocket may contain the authentication URL.
        row["unavailable"] = {"stage":stage,"type":type(error).__name__}
    finally:
        if socket is not None:
            socket.close()
    return row


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("output",type=Path)
    args = cli.parse_args()
    rows = [sample(*inputs) for inputs in [("ETH_USDT","SELL","0.04",True),
            ("ETH_USDT","BUY","0.04",False),("USDC_USDT","SELL","100",False)]]
    args.output.write_text(json.dumps({"sdk":"@tokenlon/sdk@5.4.2","results":rows},indent=2),encoding="utf8")
