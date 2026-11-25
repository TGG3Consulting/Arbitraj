"""Prepare an anonymous cash-flow projection. Input and output belong on the analyst's machine."""
import argparse
import gzip
import hashlib
import json
from collections import Counter
from datetime import datetime, timedelta
from decimal import Decimal
from pathlib import Path


def timestamp(text):
    return datetime.fromisoformat(text.replace("Z", "+00:00"))


def read_cards(path):
    raw = path.read_bytes()
    if raw.startswith(b"\x1f\x8b"):
        raw = gzip.decompress(raw)
    cards = json.loads(raw)
    return {card["id"]: card for card in cards}


def end_at(card):
    for when, note in reversed(card["timeline"]):
        if note.startswith(("Сделка закрыта:", "Closed: ")):
            return timestamp(when)
    return None


def financial_part(part):
    if part is None:
        return None
    out = {k: part[k] for k in ("state", "amount", "sent", "credited", "fee_usd", "tries") if k in part}
    out.update({k: "sample" for k in ("at", "by", "why", "from", "to") if k in part})
    return out


def correction_map(receipt, original):
    # Only these request IDs were verified in the gross/fee journal and the private ledger.
    keys = ("20261124052119344TLOS", "20261124052115166TLOS")
    address = "0xdfcbe6dd5056c896066db400f414e2c353c095e9"
    token = "0x55d398326f99059ff775485246999027b3197955"
    assert receipt["transactionHash"] == "0x2901e120419968af20f125154053f1f3532e966eeb5cfa7ed8d14f07fcd1a22f"
    assert int(receipt["blockNumber"], 16) == 135406118 and int(receipt["status"], 16) == 1
    candidates = {int(log["logIndex"], 16): Decimal(int(log["data"], 16)).scaleb(-18)
                  for log in receipt["logs"] if log["address"].lower() == token
                  and len(log["topics"]) == 3 and log["topics"][0].lower() ==
                  "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"
                  and log["topics"][2][-40:].lower() == address[2:]}
    assert set(candidates) == {143, 144}
    corrected = {}
    for key in keys:
        part = original[key]["usdt_return"]
        assert Decimal(part["credited"]) == sum(candidates.values())
        expected = Decimal(part["sent"]) - Decimal(1)
        matches = [value for value in candidates.values() if value == expected]
        assert len(matches) == 1
        corrected[key] = str(matches[0])
    assert len(set(corrected.values())) == len(keys)
    return corrected


def project(card, corrected, cutoff):
    when = end_at(card)
    groups = ["all"]
    if when:
        if cutoff - timedelta(days=1) <= when < cutoff:
            groups.append("current_24")
        if cutoff - timedelta(days=2) <= when < cutoff - timedelta(days=1):
            groups.append("previous_24")
        if cutoff.replace(hour=0, minute=0, second=0, microsecond=0) <= when < cutoff:
            groups.append("utc_day")
    parts = [financial_part(x) for x in card.get("usdt_return_parts", [])]
    group = card.get("group") or {}
    return dict(
        groups=groups,
        has_end=bool(when),
        invested=card["invested"], proceeds=card["proceeds"], spent=card["spent"],
        recorded=card.get("result_usd"), return_cost=card.get("return_cost_usd"),
        first=financial_part(card.get("usdt_return")), rest=parts,
        pieces=group.get("pieces"), verified_credit=corrected.get(card["id"]),
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("original", "latest", "receipt", "output"):
        parser.add_argument(name, type=Path)
    parser.add_argument("--cutoff", required=True, type=timestamp)
    args = parser.parse_args()
    original, latest = read_cards(args.original), read_cards(args.latest)
    assert original.keys() <= latest.keys()
    corrected = correction_map(json.loads(args.receipt.read_bytes()), original)
    # Original and latest keep their own observed state; avoid applying newer updates retroactively.
    result = {}
    for name, cards in (("original", original), ("latest", latest)):
        rows = []
        for key in sorted(cards, key=lambda k: hashlib.sha256(k.encode()).digest()):
            card = cards[key]
            assert card["step"] == "Closed"
            rows.append(project(card, corrected, args.cutoff))
        result[name] = rows
        print(name, len(rows), Counter(g for row in rows for g in row["groups"]))
    with gzip.open(args.output, "wt", encoding="utf-8", compresslevel=7) as f:
        json.dump(result, f, separators=(",", ":"))


if __name__ == "__main__":
    main()
