"""Сверяет Vyper исходники и реальные коды четырёх реализаций Spectra с формулами Curve."""
import difflib
import gzip
import hashlib
import json
import re
from pathlib import Path

import requests
from eth_utils import keccak
from record_spectra_base import AT, read

DATA = Path("tests/data/first_run_dex_2026-11-26")
SOURCE = {
    "sng": "https://raw.githubusercontent.com/curvefi/stableswap-ng/3332cd4/contracts/main/CurveStableSwapNG.vy",
    "legacy": "https://raw.githubusercontent.com/curvefi/curve-factory-crypto/7ed1011/contracts/CurveCryptoSwap2ETH.vy",
}


def statements(text):
    stripped = re.sub(r'""".*?"""', "", text, flags=re.S)
    return [line.split("#")[0].rstrip() for line in stripped.splitlines() if line.split("#")[0].strip()]


def get(url):
    reply = requests.get(url, timeout=20)
    reply.raise_for_status()
    return reply


def capture():
    directory = json.loads(gzip.decompress((DATA/"spectra_base_directory.json.gz").read_bytes()))
    examples = {}
    for row in directory["markets"]:
        examples.setdefault(row["template"], row)
    rows = []
    for template, r in examples.items():
        # SNG использует blueprint с аргументами immutable; Sourcify проверяет конечный код реального пула.
        address = r["pool"] if r["kind"]=="sng" else template
        url = f"https://sourcify.dev/server/v2/contract/8453/{address}?fields=sources,compilation,runtimeBytecode.onchainBytecode"
        published = get(url).json()
        runtime = read("eth_getCode", [address, AT])
        if published["runtimeMatch"] not in ("match","exact_match") or runtime.lower() != published["runtimeBytecode"]["onchainBytecode"].lower():
            raise ValueError(f"исходник не подтверждает {address} на выбранном блоке")
        source, = published["sources"].values()
        upstream = get(SOURCE[r["kind"]]).text
        delta = list(difflib.unified_diff(statements(upstream),statements(source["content"]),lineterm="",n=0))[2:]
        rows.append(dict(template=template,source_address=address,kind=r["kind"],runtime_hash="0x"+keccak(bytes.fromhex(runtime[2:])).hex(),
                         upstream=SOURCE[r["kind"]],verified=url,match=published["runtimeMatch"],
                         source_sha256=hashlib.sha256(source["content"].encode()).hexdigest(),statement_diff=delta))
    addrs = {directory["registry"]} | {r[k] for r in directory["markets"] for k in ("template","factory")}
    contracts = {}
    for addr in sorted(addrs):
        raw = bytes.fromhex(read("eth_getCode", [addr,AT])[2:])
        if not raw:raise ValueError("пустой runtime "+addr)
        contracts[addr] = dict(bytes=len(raw),keccak256="0x"+keccak(raw).hex())
    return dict(selector=AT,implementations=rows,contracts=contracts)


if __name__=="__main__":
    report=capture()
    (DATA/"spectra_base_sources.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf8")
    path=DATA/"base.json"
    base=json.loads(path.read_bytes())
    if base["selector"]!=AT:raise ValueError("первичный реестр использует другой блок")
    base["contracts"].update(report["contracts"])
    path.write_text(json.dumps(base,indent=2,ensure_ascii=False)+"\n",encoding="utf8")
