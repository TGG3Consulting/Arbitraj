"""Формирует адресные профили развёртываний Balancer V2/V3 Base/Ethereum по закреплённому манифесту.

Использует только семейства, сопоставленные с математикой существующего адаптера (решения 450, 452, 473).
Дочерние ключи V2 проверяют фабрику отдельно. Для V3 перечислены допущенные фабрики каждого выпуска.
"""
import json
from pathlib import Path

import requests
from eth_utils import keccak

ROOT = Path(__file__).resolve().parents[1]
REV = "b55dc2b8a46bce8f27aed5a3b03f719f662f860a"

# Поколения V2 с проверенной формулой и ABI. Один адрес на задачу манифеста и название фабрики.
V2_TYPES = {
    "composable_stable": "20220906 20221122 20230320-cs 20230711 20240223",
    "stable_phantom": "20211208-sp",
    "meta_stable": "20210727",
    "lbp": "20210721 20211202",
    "investment": "20210907",
    "managed": "20221021 20230411",
    "linear_v1": "20211208-aave 20220404 20220425",
    "linear": "20220817 20230206-aave 20230213-yearn 20230409-yearn 20230409-erc4626 20230410-aave",
}
# Суффикс после даты устраняет двусмысленность нескольких независимых выпусков в один день.
V3_GROUPS = {
    "weighted_factories": ("WeightedPoolFactory","20241205-weighted"),
    "weighted_min_factories": ("WeightedPoolFactory","20260115"),
    "stable_factories": ("StablePoolFactory","20241205-stable"),
    "stable_open_factories": ("StablePoolFactory|StableSurgePoolFactory","20250324 20250121 20250404"),
    "stable_guard_factories": ("StablePoolFactory|StableSurgePoolFactory","20260116 20260117"),
    "gyro_2clp_factories": ("Gyro2CLPPoolFactory","20250120 20260123"),
    "gyro_eclp_factories": ("GyroECLPPoolFactory","20250124 20260126"),
    "reclamm_factories": ("ReClammPoolFactory","20250409 20250702"),
    "reclamm_v3_factories": ("ReClammPoolFactory","20260428"),
    "lbp_v1_factories": ("LBPoolFactory","20250307"),
    "lbp_v2_factories": ("LBPoolFactory","20250701"),
    "lbp_v3_factories": ("LBPoolFactory","20251219"),
    "lbp_v4_factories": ("LBPoolFactory","20260501"),
    "fixed_lbp_factories": ("FixedPriceLBPoolFactory","20251205"),
    "stable_surge_hooks": ("StableSurgeHook","20250121 20250403"),
}


def read(path):
    """Данные JSON из профиля или предыдущего снимка."""
    return json.loads(path.read_text("utf8"))


def write(path, value):
    """Сохраняет проверенный набор после получения всех сетевых ответов."""
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf8", newline="\n")


def entries(network, manifest):
    """Разрешает именованные фабрики, сохраняя официальные имена задач и отдельно неподдержанные выпуски."""
    b=network=="base"
    parent="balancer-v2-base" if b else "balancer_ethereum"
    tag="base" if b else "mainnet"
    source=f"https://github.com/balancer/balancer-deployments/blob/{REV}/addresses/{tag}.json"
    result=[]
    def profile(id, settings, release):
        row=dict(chain_id=8453 if b else 1, network=network, id=id, source=source+" · "+release, settings=settings)
        result.append(row)
        return row
    contracts={(task,c["name"]):c["address"] for task,r in manifest.items() for c in r["contracts"]}
    vault=contracts["20210418-vault","Vault"]
    primary="20230320-weighted-pool-v4" if b else "20210418-weighted-pool"
    def weighted(task,name="WeightedPoolFactory"):
        conf={"dex.kind":"balancer_v2","dex.vault":vault,"dex.factory":contracts[task,name]}
        if task!="20210418-weighted-pool":
            conf["dex.balancer_math"]="v2"
        if task in ("20210418-weighted-pool","20220908-weighted-pool-v2") and name=="WeightedPoolFactory":
            stable="20210624-stable-pool" if task==primary else "20220609-stable-pool-v2"
            conf["dex.balancer_stable_factory"]=contracts[stable,"StablePoolFactory"]
        return conf
    main=profile(parent,weighted(primary),primary)
    children=[]
    for (task,name),address in contracts.items():
        if manifest[task]["version"]!="v2" or (task,name)==(primary,"WeightedPoolFactory"): continue
        if name=="WeightedPool2TokensFactory" or name=="WeightedPoolFactory":
            row=weighted(task,name)
        else:
            kind=None
            for k,patterns in V2_TYPES.items():
                for pattern in patterns.split():
                    date,_,part=pattern.partition("-")
                    part={"cs":"composable","sp":"stable-phantom"}.get(part,part)
                    if task.startswith(date+"-") and (not part or part in task) and name.endswith("PoolFactory"):
                        kind=k
            if kind is None: continue
            row={"dex.kind":"balancer_v2","dex.vault":vault,"dex.factory":address,"dex.balancer_pool":kind}
        id="bal-v2-"+address[2:14].lower()
        children.append(id)
        profile(id,row,task+" · "+name)
    main["settings"]["dex.sub_dex"]=children
    conf={"dex.kind":"balancer_v3","dex.vault":contracts["20241204-v3-vault","Vault"],
          "dex.quoter":contracts["20250307-v3-router-v2","Router"],
          "dex.bv3_batch_router":contracts["20241205-v3-batch-router","BatchRouter"]}
    for kind,(names,patterns) in V3_GROUPS.items():
        found=[]
        for pattern in patterns.split():
            date,_,part=pattern.partition("-")
            hits=[address for (task,name),address in contracts.items()
                  if manifest[task]["version"]=="v3" and task.startswith(date+"-") and part in task
                  and name in names.split("|")]
            if len(hits)>1: raise ValueError((network,kind,pattern,hits))
            found.extend(hits)
        conf["dex.bv3_"+kind]=found
    # Пустая разрешительная выборка для этих выпусков. MEV Capture требует проверки семантики чаевых
    # конкретной сети (сейчас обработчик использует ArbOS). QuantAMM требует отдельного манифеста команды.
    conf["dex.bv3_mev_capture_hooks"]=[]
    conf["dex.bv3_quantamm_factories"]=[]
    profile("balancer-v3-"+network,conf,"V3 · фабрики по выпускам")
    return result


def main():
    """Обновляет набор только после непустого eth_getCode на каноническом блоке для каждого адреса."""
    target=ROOT/"config/first_run_dex.json"
    profiles=[r for r in read(target) if r["settings"].get("dex.kind") not in ("balancer_v2","balancer_v3")]
    collected=[]
    for net,chain in [("base","base"),("ethereum","mainnet")]:
        r=requests.get(f"https://raw.githubusercontent.com/balancer/balancer-deployments/{REV}/addresses/{chain}.json",timeout=20)
        r.raise_for_status()
        additions=entries(net,r.json())
        evidence_path=ROOT/f"tests/data/first_run_dex_2026-11-26/{net}.json"
        evidence=read(evidence_path)
        addresses={v.lower() for p in additions for raw in p["settings"].values()
                   for v in (raw if isinstance(raw,list) else [raw]) if v.startswith("0x")}
        addresses=sorted(addresses)
        for start in range(0,len(addresses),3):
            addrs=addresses[start:start+3]
            req=[dict(id=i,method="eth_getCode",jsonrpc="2.0",params=[a,evidence["selector"]]) for i,a in enumerate(addrs)]
            response=requests.post(evidence["endpoint"],json=req,timeout=20)
            response.raise_for_status()
            got={item["id"]:item for item in response.json()}
            for i,a in enumerate(addrs):
                code=bytes.fromhex(got[i]["result"][2:])
                if not code: raise ValueError(f"{net} {a}: пустой runtime")
                evidence["contracts"][a]={"bytes":len(code),"keccak256":"0x"+keccak(code).hex()}
        profiles.extend(additions)
        collected.append((evidence_path,evidence))
        print(net,len(additions),len(addresses))
    for path,evidence in collected: write(path,evidence)
    write(target,profiles)


if __name__=="__main__":
    main()
